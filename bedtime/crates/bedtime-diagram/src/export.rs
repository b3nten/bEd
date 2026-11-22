//! A diagram as a picture: SVG, written here, with no drawing library. (A
//! PNG is that SVG rendered: see `lib.rs`.) It's drawn as the canvas draws
//! it, but for the font, which is whichever of the viewer's it finds, and
//! labels, which break only where they were written to.
use crate::edit;
use crate::model::{Color, Dash, Element, GROUP_LABEL_SIZE, Head, Point, Rect, Shape, TEXT_SIZE};
use std::fmt::Write;

/// The room left around the picture, on the canvas.
const MARGIN: f32 = 24.0;
/// A line of text's height, over its size (as the canvas's font has it).
const LINE_HEIGHT: f32 = 1.33;
/// How far under a line's top its baseline is, over its size.
const BASELINE: f32 = 0.97;
/// How wide a letter is, over its size: for the room behind a line's label.
const LETTER: f32 = 0.6;
/// How much of a fill's color shows (the canvas's tint).
const FILL_OPACITY: f32 = 0.22;
/// How high a line's label is (as on the canvas).
const LINE_LABEL_SIZE: f32 = 16.0;

/// The colors to draw in: the theme's, as red, green and blue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inks {
    /// What's behind everything.
    pub background: [u8; 3],
    /// Each named color, in `Color::ALL`'s order.
    pub named: [[u8; 3]; 9],
}

impl Inks {
    fn of(&self, color: Color) -> String {
        let index = Color::ALL
            .iter()
            .position(|named| *named == color)
            .unwrap_or(0);
        hex(self.named[index])
    }
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Text as it's written in XML.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A number as it's written: to a hundredth, without a trailing ".0".
fn number(value: f32) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded == 0.0 {
        "0".into()
    } else {
        format!("{rounded}")
    }
}

fn points(points: &[Point]) -> String {
    points
        .iter()
        .map(|point| format!("{},{}", number(point.x), number(point.y)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The picture of some elements (back to front), `scale` times their size on
/// the canvas, on the background. `None` for no elements.
pub fn svg(elements: &[&Element], inks: &Inks, scale: f32) -> Option<String> {
    let area = elements
        .iter()
        .map(|element| element.bounds())
        .reduce(|all, bounds| all.union(&bounds))?
        .expand(MARGIN);
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="{} {} {} {}" font-family="Bedfont, ui-monospace, Menlo, Consolas, monospace">"#,
        number(area.w * scale),
        number(area.h * scale),
        number(area.x),
        number(area.y),
        number(area.w),
        number(area.h),
    );
    let _ = writeln!(
        out,
        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
        number(area.x),
        number(area.y),
        number(area.w),
        number(area.h),
        hex(inks.background),
    );
    for element in elements {
        draw(&mut out, element, inks);
    }
    out.push_str("</svg>\n");
    Some(out)
}

/// The attributes of an element's stroke.
fn stroke(element: &Element, inks: &Inks) -> String {
    let width = element.width;
    let dashes = match element.dash {
        Dash::Solid => String::new(),
        Dash::Dashed => format!(
            r#" stroke-dasharray="{} {}""#,
            number(width * 4.0 + 2.0),
            number(width * 3.0 + 1.0)
        ),
        Dash::Dotted => format!(
            r#" stroke-dasharray="{} {}""#,
            number(width),
            number(width * 2.0 + 1.0)
        ),
    };
    format!(
        r#"stroke="{}" stroke-width="{}"{dashes}"#,
        inks.of(element.stroke),
        number(width)
    )
}

/// The attributes of a box's fill.
fn fill(element: &Element, inks: &Inks) -> String {
    match element.fill {
        Some(color) => format!(r#"fill="{}" fill-opacity="{FILL_OPACITY}""#, inks.of(color)),
        None => r#"fill="none""#.into(),
    }
}

fn draw(out: &mut String, element: &Element, inks: &Inks) {
    let stroke = stroke(element, inks);
    match &element.shape {
        Shape::Rectangle(rect) => {
            let radius = if element.rounded {
                edit::corner_radius(rect)
            } else {
                0.0
            };
            let _ = writeln!(
                out,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" {} {stroke}/>"#,
                number(rect.x),
                number(rect.y),
                number(rect.w),
                number(rect.h),
                number(radius),
                fill(element, inks),
            );
        }
        Shape::Ellipse(rect) => {
            let center = rect.center();
            let _ = writeln!(
                out,
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" {} {stroke}/>"#,
                number(center.x),
                number(center.y),
                number(rect.w / 2.0),
                number(rect.h / 2.0),
                fill(element, inks),
            );
        }
        Shape::Diamond(rect) => {
            let center = rect.center();
            let corners = [
                Point::new(center.x, rect.y),
                Point::new(rect.right(), center.y),
                Point::new(center.x, rect.bottom()),
                Point::new(rect.x, center.y),
            ];
            let _ = writeln!(
                out,
                r#"<polygon points="{}" {} {stroke}/>"#,
                points(&corners),
                fill(element, inks),
            );
        }
        Shape::Line {
            points: through,
            start,
            end,
            ..
        } => {
            let _ = writeln!(
                out,
                r#"<polyline points="{}" fill="none" {stroke}/>"#,
                points(through)
            );
            // Heads are solid, whatever the line's dashes.
            let solid = format!(
                r#"stroke="{}" stroke-width="{}""#,
                inks.of(element.stroke),
                number(element.width)
            );
            let reach = 8.0 + 3.0 * element.width;
            if let [first, second, ..] = through[..]
                && *start == Head::Arrow
            {
                head(out, second, first, reach, &solid);
            }
            if let [.., before, last] = through[..]
                && *end == Head::Arrow
            {
                head(out, before, last, reach, &solid);
            }
        }
        Shape::Freehand { points: through } => {
            let _ = writeln!(
                out,
                r#"<polyline points="{}" fill="none" stroke-linejoin="round" stroke-linecap="round" {stroke}/>"#,
                points(&edit::smooth(through, 2.0))
            );
        }
        Shape::Text { at, text, size } => {
            lines(
                out,
                text,
                at.x,
                at.y,
                *size,
                "start",
                &inks.of(element.stroke),
            );
        }
        // Drawn only with a label: a frame, and the label at its top left.
        Shape::Group { around, .. } => {
            if element.label.is_empty() {
                return;
            }
            let _ = writeln!(
                out,
                r#"<rect x="{}" y="{}" width="{}" height="{}" {} {stroke}/>"#,
                number(around.x),
                number(around.y),
                number(around.w),
                number(around.h),
                fill(element, inks),
            );
            lines(
                out,
                &element.label,
                around.x + 8.0,
                around.y + 6.0,
                GROUP_LABEL_SIZE,
                "start",
                &inks.of(element.stroke),
            );
            return;
        }
    }
    if element.label.is_empty() {
        return;
    }
    let ink = inks.of(element.stroke);
    let count = element.label.lines().count().max(1) as f32;
    if element.is_box() {
        let Rect { y, h, .. } = element.bounds();
        let middle = element.bounds().center().x;
        let top = y + h / 2.0 - count * TEXT_SIZE * LINE_HEIGHT / 2.0;
        lines(out, &element.label, middle, top, TEXT_SIZE, "middle", &ink);
    } else if let Shape::Line {
        points: through, ..
    } = &element.shape
        && let Some(middle) = edit::midpoint(through)
    {
        // Over the line, on a patch of the background.
        let size = LINE_LABEL_SIZE;
        let widest = element
            .label
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        let (width, height) = (
            widest as f32 * size * LETTER + 6.0,
            count * size * LINE_HEIGHT + 6.0,
        );
        let _ = writeln!(
            out,
            r#"<rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="{}"/>"#,
            number(middle.x - width / 2.0),
            number(middle.y - height / 2.0),
            number(width),
            number(height),
            hex(inks.background),
        );
        let top = middle.y - count * size * LINE_HEIGHT / 2.0;
        lines(out, &element.label, middle.x, top, size, "middle", &ink);
    }
}

/// Lines of text from `top` down, each anchored at `x`.
fn lines(out: &mut String, text: &str, x: f32, top: f32, size: f32, anchor: &str, ink: &str) {
    let _ = write!(
        out,
        r#"<text font-size="{}" text-anchor="{anchor}" fill="{ink}" xml:space="preserve">"#,
        number(size)
    );
    for (index, line) in text.lines().enumerate() {
        let baseline = top + size * (BASELINE + LINE_HEIGHT * index as f32);
        let _ = write!(
            out,
            r#"<tspan x="{}" y="{}">{}</tspan>"#,
            number(x),
            number(baseline),
            escaped(line)
        );
    }
    out.push_str("</text>\n");
}

/// An arrow's head at `tip`, for a line arriving from `from` (as the canvas
/// draws it).
fn head(out: &mut String, from: Point, tip: Point, reach: f32, stroke: &str) {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let length = dx.hypot(dy);
    if length < f32::EPSILON {
        return;
    }
    let reach = reach.min(length * 0.5);
    let (back_x, back_y) = (-dx / length * reach, -dy / length * reach);
    let (sin, cos) = 0.45_f32.sin_cos();
    let barb = |sin: f32| {
        Point::new(
            tip.x + back_x * cos - back_y * sin,
            tip.y + back_x * sin + back_y * cos,
        )
    };
    let _ = writeln!(
        out,
        r#"<polyline points="{}" fill="none" {stroke}/>"#,
        points(&[barb(sin), tip, barb(-sin)])
    );
}
