//! Colors written in text: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(…)`,
//! `rgba(…)`, `hsl(…)` and `hsla(…)`. The editor underlines each in its color
//! and, on hover, offers a picker that writes the new color back in the same
//! form.
use bedtime_ui::egui::{self, ecolor::Hsva};

/// sRGB channels and alpha, 0 to 1, alpha not premultiplied.
pub(crate) type Srgba = [f32; 4];
use std::ops::Range;

/// A color in a line of text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Literal {
    /// Bytes in the line it was found in.
    pub range: Range<usize>,
    pub color: Srgba,
    pub form: Form,
}

/// How a color is written, so a new one is written the same way.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Form {
    /// `digits` is 3, 4, 6 or 8.
    Hex { digits: usize, upper: bool },
    /// `rgb(…)`/`rgba(…)` (`hsl` when `hsl`), as named (`name` keeps its
    /// case), with commas or (CSS 4) spaces and a `/` before the alpha.
    Function {
        name: String,
        hsl: bool,
        alpha: bool,
        commas: bool,
    },
}

impl Form {
    pub fn has_alpha(&self) -> bool {
        match self {
            Form::Hex { digits, .. } => matches!(digits, 4 | 8),
            Form::Function { alpha, .. } => *alpha,
        }
    }
}

/// Lines longer than this are only searched near what's shown.
pub(crate) const LONGEST_LINE: usize = 4096;

/// Every color written in `line`.
pub(crate) fn find(line: &str) -> Vec<Literal> {
    let bytes = line.as_bytes();
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let starts_word = before.is_none_or(|b| !word(b) && b != b'&' && b != b'#');
        let literal = if bytes[at] == b'#' && starts_word {
            hex(line, at)
        } else if starts_word && bytes[at].is_ascii_alphabetic() {
            function(line, at)
        } else {
            None
        };
        match literal {
            Some(literal) => {
                at = literal.range.end;
                found.push(literal);
            }
            None => at += 1,
        }
    }
    found
}

fn hex(line: &str, at: usize) -> Option<Literal> {
    let digits = line[at + 1..]
        .bytes()
        .take_while(u8::is_ascii_hexdigit)
        .count();
    let end = at + 1 + digits;
    // `#define`, `#abcdefg`: not a color.
    if !matches!(digits, 3 | 4 | 6 | 8)
        || line[end..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    let text = &line[at + 1..end];
    let nibble = |i: usize| u8::from_str_radix(&text[i..i + 1], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&text[i..i + 2], 16).ok();
    let [r, g, b, a] = match digits {
        3 | 4 => {
            let channel = |i: usize| nibble(i).map(|v| v * 17);
            [
                channel(0)?,
                channel(1)?,
                channel(2)?,
                if digits == 4 { channel(3)? } else { 255 },
            ]
        }
        _ => [
            pair(0)?,
            pair(2)?,
            pair(4)?,
            if digits == 8 { pair(6)? } else { 255 },
        ],
    };
    Some(Literal {
        range: at..end,
        color: [r, g, b, a].map(|v| v as f32 / 255.0),
        form: Form::Hex {
            digits,
            upper: text.bytes().any(|b| b.is_ascii_uppercase()),
        },
    })
}

fn function(line: &str, at: usize) -> Option<Literal> {
    let name_len = line[at..]
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .count();
    let name = &line[at..at + name_len];
    let lower = name.to_ascii_lowercase();
    let (hsl, named_alpha) = match lower.as_str() {
        "rgb" => (false, false),
        "rgba" => (false, true),
        "hsl" => (true, false),
        "hsla" => (true, true),
        _ => return None,
    };
    let open = at + name_len;
    if line[open..].bytes().next() != Some(b'(') {
        return None;
    }
    let close = open + line[open..].bytes().take(96).position(|b| b == b')')?;
    let inside = &line[open + 1..close];
    let commas = inside.contains(',');
    let (channels, alpha) = match inside.split_once('/') {
        Some((channels, alpha)) if !commas => (channels, Some(alpha)),
        Some(_) => return None,
        None => (inside, None),
    };
    let mut values: Vec<&str> = if commas {
        channels.split(',').map(str::trim).collect()
    } else {
        channels.split_whitespace().collect()
    };
    let alpha = match alpha {
        Some(alpha) => Some(alpha.trim()),
        None if values.len() == 4 => values.pop(),
        None => None,
    };
    if values.len() != 3 || values.iter().any(|v| v.is_empty()) {
        return None;
    }
    let a = match alpha {
        Some(alpha) => fraction(alpha, 1.0)?,
        None => 1.0,
    };
    let color = if hsl {
        let h = values[0].trim_end_matches("deg").parse::<f32>().ok()? / 360.0;
        let s = fraction(values[1], 100.0)?;
        let l = fraction(values[2], 100.0)?;
        let [r, g, b] = hsl_to_rgb(h.rem_euclid(1.0), s, l);
        [r, g, b, a]
    } else {
        let channel = |v: &str| fraction(v, 255.0);
        [
            channel(values[0])?,
            channel(values[1])?,
            channel(values[2])?,
            a,
        ]
    };
    Some(Literal {
        range: at..close + 1,
        color,
        form: Form::Function {
            name: name.to_owned(),
            hsl,
            alpha: named_alpha || alpha.is_some(),
            commas,
        },
    })
}

/// `v` as 0..=1: a percentage, or a number out of `scale`.
fn fraction(v: &str, scale: f32) -> Option<f32> {
    let value = match v.strip_suffix('%') {
        Some(percent) => percent.trim().parse::<f32>().ok()? / 100.0,
        None => v.parse::<f32>().ok()? / scale,
    };
    value.is_finite().then(|| value.clamp(0.0, 1.0))
}

/// `color` written as `form` writes it.
pub(crate) fn write(color: Srgba, form: &Form) -> String {
    let [r, g, b, a] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    match form {
        Form::Hex { digits, upper } => {
            let short = matches!(digits, 3 | 4) && [r, g, b, a].iter().all(|v| v % 17 == 0);
            let text = match (short, form.has_alpha()) {
                (true, false) => format!("#{:x}{:x}{:x}", r / 17, g / 17, b / 17),
                (true, true) => format!("#{:x}{:x}{:x}{:x}", r / 17, g / 17, b / 17, a / 17),
                (false, false) => format!("#{r:02x}{g:02x}{b:02x}"),
                (false, true) => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
            };
            if *upper {
                text.to_ascii_uppercase()
            } else {
                text
            }
        }
        Form::Function {
            name,
            hsl,
            alpha,
            commas,
        } => {
            let channels = if *hsl {
                let [h, s, l] = rgb_to_hsl([color[0], color[1], color[2]]);
                [
                    number(h * 360.0),
                    format!("{}%", number(s * 100.0)),
                    format!("{}%", number(l * 100.0)),
                ]
            } else {
                [r, g, b].map(|v| v.to_string())
            };
            let opacity = number(color[3]);
            match (alpha, commas) {
                (false, true) => format!("{name}({})", channels.join(", ")),
                (false, false) => format!("{name}({})", channels.join(" ")),
                (true, true) => format!("{name}({}, {opacity})", channels.join(", ")),
                (true, false) => format!("{name}({} / {opacity})", channels.join(" ")),
            }
        }
    }
}

/// A color drawn this frame, where it's shown (`range` in the document).
pub(crate) struct Hit {
    pub range: Range<usize>,
    pub literal: Literal,
    pub rect: egui::Rect,
}

/// How long the pointer rests on a color before its picker opens.
pub(crate) const REST: f64 = 0.3;

/// The color picker: which color the pointer rests on, and the open one.
#[derive(Default)]
pub(crate) struct Picker {
    resting: Option<(Range<usize>, f64)>,
    pub(crate) open: Option<Open>,
}

pub(crate) struct Open {
    /// The literal in the document, at `revision`.
    pub range: Range<usize>,
    pub revision: u64,
    pub form: Form,
    pub color: Hsva,
    pub anchor: egui::Rect,
    /// Where the popup was drawn last frame.
    pub popup: Option<egui::Rect>,
    /// A drag began in the popup: it stays open until the button's released.
    pub dragging: bool,
    /// Edits were made: they're one undo step, ended when it closes.
    pub edited: bool,
}

impl Picker {
    /// Whether `pointer` is on the open picker or its color.
    pub fn holds(&self, pointer: egui::Pos2) -> bool {
        self.open.as_ref().is_some_and(|open| {
            open.anchor.contains(pointer)
                || open.popup.is_some_and(|popup| popup.expand(6.0).contains(pointer))
                // The gap between the color and the popup below it.
                || open.popup.is_some_and(|popup| {
                    egui::Rect::from_min_max(open.anchor.left_bottom(), popup.right_top())
                        .contains(pointer)
                })
        })
    }

    /// The color under `pointer`, if it has rested there long enough to open
    /// its picker (`now` is the UI's time). Asks for a repaint while waiting.
    pub fn rest<'a>(
        &mut self,
        ctx: &egui::Context,
        hits: &'a [Hit],
        pointer: Option<egui::Pos2>,
        now: f64,
    ) -> Option<&'a Hit> {
        let Some(hit) = pointer.and_then(|p| hits.iter().find(|hit| hit.rect.contains(p))) else {
            self.resting = None;
            return None;
        };
        let since = match &self.resting {
            Some((range, since)) if *range == hit.range => *since,
            _ => {
                self.resting = Some((hit.range.clone(), now));
                now
            }
        };
        if now - since >= REST {
            self.resting = None;
            Some(hit)
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(REST - (now - since)));
            None
        }
    }
}

/// At most two decimals, without trailing zeros.
fn number(value: f32) -> String {
    let text = format!("{:.2}", value);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// For the picker.
pub(crate) fn to_hsva(color: Srgba) -> Hsva {
    let [r, g, b, a] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Hsva::from_srgba_unmultiplied([r, g, b, a])
}

/// From the picker.
pub(crate) fn from_hsva(color: Hsva) -> Srgba {
    color.to_srgba_unmultiplied().map(|v| v as f32 / 255.0)
}

/// What the underline is drawn in.
pub(crate) fn to_color32(color: Srgba) -> egui::Color32 {
    let [r, g, b, a] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h * 6.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

fn rgb_to_hsl([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    [h, s, l]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &str) -> Vec<&str> {
        find(line).into_iter().map(|l| &line[l.range]).collect()
    }

    #[test]
    fn colors_are_found_in_their_common_forms() {
        assert_eq!(
            texts("color: #fff; background: #12345678; border: #ABCDEF"),
            ["#fff", "#12345678", "#ABCDEF"]
        );
        assert_eq!(
            texts("a: rgb(255, 0, 0); b: rgba(0,0,0,0.5); c: rgb(10 20 30 / 50%)"),
            ["rgb(255, 0, 0)", "rgba(0,0,0,0.5)", "rgb(10 20 30 / 50%)"]
        );
        assert_eq!(
            texts("hsl(120deg, 50%, 25%) HSLA(0, 100%, 50%, .3)"),
            ["hsl(120deg, 50%, 25%)", "HSLA(0, 100%, 50%, .3)"]
        );
    }

    #[test]
    fn things_that_only_look_like_colors_arent() {
        assert!(texts("#define X 1").is_empty());
        assert!(texts("#[derive(Debug)] issue #12 &#123; #abcdefg").is_empty());
        assert!(texts("myrgb(1, 2, 3) rgb(1, 2) rgb(a, b, c) hsl(1 2 3, 4)").is_empty());
    }

    #[test]
    fn colors_are_written_back_as_they_were() {
        for line in [
            "#fff",
            "#ABCDEF",
            "#11223344",
            "#1234",
            "rgb(255, 0, 0)",
            "rgba(0, 0, 0, 0.5)",
            "rgb(10 20 30 / 0.5)",
            "hsl(120, 50%, 25%)",
            "HSLA(0, 100%, 50%, 0.3)",
        ] {
            let literal = &find(line)[0];
            assert_eq!(write(literal.color, &literal.form), line);
        }
    }

    #[test]
    fn a_short_hex_color_grows_when_it_has_to() {
        let literal = &find("#fff")[0];
        let red = [254.0 / 255.0, 0.0, 0.0, 1.0];
        assert_eq!(write(red, &literal.form), "#fe0000");
        let red = [1.0, 0.0, 0.0, 1.0];
        assert_eq!(write(red, &literal.form), "#f00");
    }
}

#[cfg(test)]
mod picker_tests {
    use crate::editor::tests::{focus, frame};
    use crate::editor::*;

    fn pointer(at: egui::Pos2) -> egui::Event {
        egui::Event::PointerMoved(at)
    }

    #[test]
    fn resting_on_a_color_opens_a_picker_that_rewrites_it() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("colors");
        let mut view = EditorView::default();
        let mut doc = MemoryTextSession::new("a { color: #ff0000; }\n");
        focus(&ctx, &mut view, &mut doc, id);
        let first = frame(&ctx, &mut view, &mut doc, id, vec![]);
        let font = egui::TextStyle::Monospace.resolve(&ctx.global_style());
        let cell = ctx.fonts_mut(|f| f.glyph_width(&font, ' '));
        // "a { color: " is eleven cells; the color is the next seven.
        let on_color = egui::pos2(
            first.text_rect.left() + 8.0 + cell * 14.5,
            first.text_rect.top() + 6.0 + first.caret_rect.height() * 0.5,
        );
        frame(&ctx, &mut view, &mut doc, id, vec![pointer(on_color)]);
        assert!(
            view.color_picker.open.is_none(),
            "it waits for the pointer to rest"
        );
        for _ in 0..30 {
            frame(&ctx, &mut view, &mut doc, id, vec![]);
        }
        let popup = view
            .color_picker
            .open
            .as_ref()
            .and_then(|open| open.popup)
            .expect("the picker opens");

        // Drag in the picker's color square: the text follows, as hex.
        let start = popup.center();
        let press = |at, pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut view, &mut doc, id, vec![pointer(start)]);
        frame(&ctx, &mut view, &mut doc, id, vec![press(start, true)]);
        let end = start + egui::vec2(-20.0, 15.0);
        frame(&ctx, &mut view, &mut doc, id, vec![pointer(end)]);
        frame(&ctx, &mut view, &mut doc, id, vec![press(end, false)]);
        let text = doc.snapshot().unwrap().text.to_string();
        assert!(text.starts_with("a { color: #"), "{text}");
        assert!(
            text.ends_with("; }\n") && !text.contains("#ff0000"),
            "{text}"
        );
        assert_eq!(text.len(), "a { color: #ff0000; }\n".len(), "{text}");

        // Away from both, it closes; the edits are one undo step.
        frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![pointer(egui::pos2(590.0, 290.0))],
        );
        frame(&ctx, &mut view, &mut doc, id, vec![]);
        assert!(view.color_picker.open.is_none());
        doc.history(HistoryAction::Undo).unwrap();
        assert_eq!(
            doc.snapshot().unwrap().text.to_string(),
            "a { color: #ff0000; }\n"
        );
    }
}
