//! The `.diagram` file: RON, written to be read and written by people and
//! by language models as much as by the editor. One element per line (a
//! group's on the lines after it, indented), so diffs read well.
//!
//! ```ron
//! (
//!     version: 1,
//!     elements: [
//!         (id: "api", shape: Rectangle(x: 0, y: 0, w: 160, h: 60), label: "API"),
//!         (id: "backend", shape: Group, label: "Backend", padding: 24, children: [
//!             (id: "db", shape: Ellipse(x: 0, y: 120, w: 160, h: 60), label: "Postgres"),
//!         ]),
//!         (id: "calls", shape: Arrow(from: "api", to: "backend", via: [(200, 30)]), label: "SQL"),
//!     ],
//! )
//! ```
//!
//! What follows from the rest isn't written: where a bound end meets what
//! it's bound to (a line's `from` and `to` are names, or points for ends
//! bound to nothing, and `via` the points it bends at between them), and a
//! group's box. They're worked out when it's read. Numbers are written to a
//! hundredth, without a ".0"; `Some(…)` is left out. An `id` may be left
//! out, for something nothing refers to: it's given one.
use crate::edit;
use crate::model::{
    Color, Dash, Diagram, Element, ElementId, GROUP_PADDING, Head, Point, Rect, STROKE_WIDTH,
    Shape, Side, TEXT_SIZE,
};
use bedtime_document::ByteModel;
use bedtime_filesystem::FileModel;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;

/// The file format's version: files of a later one aren't opened.
pub const VERSION: u32 = 1;

/// A number as it's written: to a hundredth, and whole numbers without a
/// ".0". Read from either.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Num(f32);

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let rounded = (self.0 * 100.0).round() / 100.0;
        if rounded.fract() == 0.0 && rounded.abs() < 1e9 {
            serializer.serialize_i64(rounded as i64)
        } else {
            serializer.serialize_f32(rounded)
        }
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Num;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a number")
            }
            fn visit_i64<E>(self, value: i64) -> Result<Num, E> {
                Ok(Num(value as f32))
            }
            fn visit_u64<E>(self, value: u64) -> Result<Num, E> {
                Ok(Num(value as f32))
            }
            fn visit_f64<E>(self, value: f64) -> Result<Num, E> {
                Ok(Num(value as f32))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Serialize, Deserialize)]
struct FilePoint(Num, Num);

impl From<Point> for FilePoint {
    fn from(point: Point) -> Self {
        Self(Num(point.x), Num(point.y))
    }
}

impl From<&FilePoint> for Point {
    fn from(FilePoint(x, y): &FilePoint) -> Self {
        Point::new(x.0, y.0)
    }
}

#[derive(Serialize, Deserialize)]
struct FileRect {
    x: Num,
    y: Num,
    w: Num,
    h: Num,
}

impl From<Rect> for FileRect {
    fn from(rect: Rect) -> Self {
        Self {
            x: Num(rect.x),
            y: Num(rect.y),
            w: Num(rect.w),
            h: Num(rect.h),
        }
    }
}

impl From<&FileRect> for Rect {
    fn from(rect: &FileRect) -> Self {
        Rect::new(rect.x.0, rect.y.0, rect.w.0, rect.h.0)
    }
}

/// Where a line's end is: on an element, by name, or at a point.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum End {
    On(String),
    At(FilePoint),
}

#[derive(Serialize, Deserialize)]
enum FileShape {
    Rectangle(FileRect),
    Ellipse(FileRect),
    Diamond(FileRect),
    /// A line with an arrow at its end: `Line` with `end: Arrow`.
    Arrow {
        from: End,
        to: End,
        #[serde(default, skip_serializing_if = "is_default")]
        from_side: Side,
        #[serde(default, skip_serializing_if = "is_default")]
        to_side: Side,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        via: Vec<FilePoint>,
        #[serde(default, skip_serializing_if = "is_default")]
        start: Head,
    },
    Line {
        from: End,
        to: End,
        #[serde(default, skip_serializing_if = "is_default")]
        from_side: Side,
        #[serde(default, skip_serializing_if = "is_default")]
        to_side: Side,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        via: Vec<FilePoint>,
        #[serde(default, skip_serializing_if = "is_default")]
        start: Head,
        #[serde(default, skip_serializing_if = "is_default")]
        end: Head,
    },
    Freehand {
        points: Vec<FilePoint>,
    },
    Text {
        at: FilePoint,
        text: String,
        #[serde(default = "text_size", skip_serializing_if = "is_text_size")]
        size: Num,
    },
    /// Its elements are the element's `children`.
    Group,
}

#[derive(Serialize, Deserialize)]
struct FileElement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    shape: FileShape,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    label: String,
    #[serde(default, skip_serializing_if = "is_default")]
    stroke: Color,
    #[serde(default, skip_serializing_if = "is_default")]
    fill: Option<Color>,
    #[serde(default = "stroke_width", skip_serializing_if = "is_stroke_width")]
    width: Num,
    #[serde(default, skip_serializing_if = "is_default")]
    dash: Dash,
    #[serde(default, skip_serializing_if = "is_default")]
    rounded: bool,
    /// A group's room around its elements, when it isn't the usual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    padding: Option<Num>,
    /// A group's elements.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<FileElement>,
}

fn text_size() -> Num {
    Num(TEXT_SIZE)
}

fn is_text_size(size: &Num) -> bool {
    size.0 == TEXT_SIZE
}

fn stroke_width() -> Num {
    Num(STROKE_WIDTH)
}

fn is_stroke_width(width: &Num) -> bool {
    width.0 == STROKE_WIDTH
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// The file, as RON reads it.
#[derive(Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    elements: Vec<FileElement>,
}

/// How the files are read and written: `Some(…)` left out, and a shape's box
/// in the shape's own brackets (`Rectangle(x: 0, …)`).
fn ron_options() -> ron::Options {
    ron::Options::default().with_default_extension(
        ron::extensions::Extensions::UNWRAP_VARIANT_NEWTYPES
            | ron::extensions::Extensions::IMPLICIT_SOME,
    )
}

// ── Reading ──────────────────────────────────────────────────────────────────

/// A line's ends as read, to be found by name once everything's read.
struct Ends {
    index: usize,
    from: End,
    to: End,
}

/// The elements of a file, flattened back to front (a group before its
/// own), with lines' ends still to find.
fn flatten(
    elements: Vec<FileElement>,
    group: Option<ElementId>,
    out: &mut Vec<Element>,
    ends: &mut Vec<Ends>,
) -> Result<(), String> {
    for file in elements {
        let id = ElementId(out.len() as u64 + 1);
        let what = || match &file.id {
            Some(name) => format!("\"{name}\""),
            None => format!("Element {}", id.0),
        };
        let mut line_ends = None;
        let shape = match file.shape {
            FileShape::Rectangle(rect) => Shape::Rectangle((&rect).into()),
            FileShape::Ellipse(rect) => Shape::Ellipse((&rect).into()),
            FileShape::Diamond(rect) => Shape::Diamond((&rect).into()),
            FileShape::Arrow {
                from,
                to,
                from_side,
                to_side,
                via,
                start,
            } => {
                line_ends = Some((from, to));
                line(via, start, Head::Arrow, from_side, to_side)
            }
            FileShape::Line {
                from,
                to,
                from_side,
                to_side,
                via,
                start,
                end,
            } => {
                line_ends = Some((from, to));
                line(via, start, end, from_side, to_side)
            }
            FileShape::Freehand { points } => Shape::Freehand {
                points: points.iter().map(Point::from).collect(),
            },
            FileShape::Text { at, text, size } => Shape::Text {
                at: (&at).into(),
                text,
                size: size.0,
            },
            FileShape::Group => {
                if group.is_some() {
                    return Err(format!("{} is a group in a group", what()));
                }
                Shape::Group {
                    padding: file.padding.map_or(GROUP_PADDING, |padding| padding.0),
                    around: Rect::default(),
                }
            }
        };
        if file.padding.is_some() && !matches!(shape, Shape::Group { .. }) {
            return Err(format!("{} has padding, but isn't a group", what()));
        }
        let is_group = matches!(shape, Shape::Group { .. });
        if !file.children.is_empty() && !is_group {
            return Err(format!("{} has children, but isn't a group", what()));
        }
        if file.id.as_deref() == Some("") {
            return Err(format!("Element {} has an empty id", id.0));
        }
        if let Some((from, to)) = line_ends {
            ends.push(Ends {
                index: out.len(),
                from,
                to,
            });
        }
        out.push(Element {
            id,
            name: file.id.unwrap_or_default(),
            shape,
            label: file.label,
            stroke: file.stroke,
            fill: file.fill,
            width: file.width.0,
            dash: file.dash,
            rounded: file.rounded,
            group,
        });
        if is_group {
            flatten(file.children, Some(id), out, ends)?;
        }
    }
    Ok(())
}

fn line(via: Vec<FilePoint>, start: Head, end: Head, from_side: Side, to_side: Side) -> Shape {
    Shape::Line {
        // The ends are put in when they're found.
        points: via.iter().map(Point::from).collect(),
        start,
        end,
        from: None,
        to: None,
        from_side,
        to_side,
    }
}

/// Find the lines' ends: points, or the elements of those names (where
/// they're put for now: settling puts them on their outlines).
fn join(elements: &mut [Element], ends: Vec<Ends>) -> Result<(), String> {
    let names: HashMap<String, (ElementId, Point)> = elements
        .iter()
        .filter(|element| !element.name.is_empty())
        .map(|element| {
            (
                element.name.clone(),
                (element.id, element.bounds().center()),
            )
        })
        .collect();
    for Ends { index, from, to } in ends {
        let line = &mut elements[index];
        let what = if line.name.is_empty() {
            format!("Element {}", line.id.0)
        } else {
            format!("\"{}\"", line.name)
        };
        let own = line.id;
        let find = |end: &End| match end {
            End::At(point) => Ok((None, Point::from(point))),
            End::On(name) => match names.get(name) {
                Some((id, _)) if *id == own => Err(format!("{what} is bound to itself")),
                Some((id, middle)) => Ok((Some(*id), *middle)),
                None => Err(format!(
                    "{what} is bound to \"{name}\", but nothing is called that"
                )),
            },
        };
        let (first, start) = find(&from)?;
        let (last, finish) = find(&to)?;
        if let Shape::Line {
            points,
            from,
            to,
            from_side,
            to_side,
            ..
        } = &mut line.shape
        {
            points.insert(0, start);
            points.push(finish);
            (*from, *to) = (first, last);
            if first.is_none() {
                *from_side = Side::Auto;
            }
            if last.is_none() {
                *to_side = Side::Auto;
            }
        }
    }
    Ok(())
}

// ── Writing ──────────────────────────────────────────────────────────────────

fn to_file(element: &Element, names: &HashMap<ElementId, &str>) -> FileElement {
    let end = |bound: Option<ElementId>, at: Point| match bound.and_then(|id| names.get(&id)) {
        Some(name) => End::On((*name).to_owned()),
        None => End::At(at.into()),
    };
    let shape = match &element.shape {
        Shape::Rectangle(rect) => FileShape::Rectangle((*rect).into()),
        Shape::Ellipse(rect) => FileShape::Ellipse((*rect).into()),
        Shape::Diamond(rect) => FileShape::Diamond((*rect).into()),
        Shape::Line {
            points,
            start,
            end: head,
            from,
            to,
            from_side,
            to_side,
        } => {
            let first = points.first().copied().unwrap_or_default();
            let last = points.last().copied().unwrap_or_default();
            let (from, to) = (end(*from, first), end(*to, last));
            let bound = |end: &End| matches!(end, End::On(_));
            let from_side = if bound(&from) { *from_side } else { Side::Auto };
            let to_side = if bound(&to) { *to_side } else { Side::Auto };
            let via = points
                .get(1..points.len().saturating_sub(1))
                .unwrap_or_default()
                .iter()
                .map(|point| (*point).into())
                .collect();
            if *head == Head::Arrow {
                FileShape::Arrow {
                    from,
                    to,
                    from_side,
                    to_side,
                    via,
                    start: *start,
                }
            } else {
                FileShape::Line {
                    from,
                    to,
                    from_side,
                    to_side,
                    via,
                    start: *start,
                    end: *head,
                }
            }
        }
        Shape::Freehand { points } => FileShape::Freehand {
            points: points.iter().map(|point| (*point).into()).collect(),
        },
        Shape::Text { at, text, size } => FileShape::Text {
            at: (*at).into(),
            text: text.clone(),
            size: Num(*size),
        },
        Shape::Group { .. } => FileShape::Group,
    };
    let padding = match element.shape {
        Shape::Group { padding, .. } if padding != GROUP_PADDING => Some(Num(padding)),
        _ => None,
    };
    FileElement {
        id: Some(element.name.clone()).filter(|name| !name.is_empty()),
        shape,
        label: element.label.clone(),
        stroke: element.stroke,
        fill: element.fill,
        width: Num(element.width),
        dash: element.dash,
        rounded: element.rounded,
        padding,
        children: Vec::new(),
    }
}

/// An element on one line, as RON writes it.
fn one_line(element: &FileElement) -> std::io::Result<String> {
    let line = ron::ser::PrettyConfig::new()
        .compact_structs(true)
        .compact_arrays(true)
        .separate_tuple_members(false);
    let text = ron_options()
        .to_string_pretty(element, line)
        .map_err(std::io::Error::other)?;
    // RON escapes `'` in strings, which needn't be: "it's", not "it\'s".
    // (Every `\'` written is one: a backslash itself is written `\\`.)
    Ok(text.replace("\\'", "'"))
}

impl ByteModel for Diagram {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "It isn't UTF-8 text".to_owned())?;
        // An empty file is an empty diagram: `touch new.diagram`.
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let file: File = ron_options()
            .from_str(text)
            .map_err(|error| format!("It isn't a diagram: {error}"))?;
        if file.version > VERSION {
            return Err(format!(
                "It's a version {} diagram; this reads up to version {VERSION}",
                file.version
            ));
        }
        let (mut elements, mut ends) = (Vec::new(), Vec::new());
        flatten(file.elements, None, &mut elements, &mut ends)?;
        let mut names: Vec<&str> = elements
            .iter()
            .map(|element| element.name.as_str())
            .filter(|name| !name.is_empty())
            .collect();
        names.sort_unstable();
        if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(format!("Two elements have the id \"{}\"", pair[0]));
        }
        join(&mut elements, ends)?;
        let diagram = Self::new(elements)?;
        // With the ends on their outlines and the groups around their own,
        // as far as the text's size can be guessed at: the canvas measures it.
        Ok(edit::settled(&diagram, &Element::bounds))
    }

    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        let names: HashMap<ElementId, &str> = self
            .elements()
            .iter()
            .map(|element| (element.id, element.name.as_str()))
            .filter(|(_, name)| !name.is_empty())
            .collect();
        let in_group = |element: &Element| {
            element
                .group
                .and_then(|group| self.get(group))
                .is_some_and(Element::is_group)
        };
        writeln!(out, "(")?;
        writeln!(out, "    version: {VERSION},")?;
        writeln!(out, "    elements: [")?;
        for element in self.elements().iter().filter(|element| !in_group(element)) {
            let line = one_line(&to_file(element, &names))?;
            let members: Vec<&Element> = if element.is_group() {
                self.members(element.id).collect()
            } else {
                Vec::new()
            };
            if members.is_empty() {
                writeln!(out, "        {line},")?;
                continue;
            }
            // `(…)` as `(…, children: [`, its own a line each, and `]),`.
            let open = line.strip_suffix(')').unwrap_or(&line);
            writeln!(out, "        {open}, children: [")?;
            for member in members {
                writeln!(out, "            {},", one_line(&to_file(member, &names))?)?;
            }
            writeln!(out, "        ]),")?;
        }
        writeln!(out, "    ],")?;
        writeln!(out, ")")
    }
}

impl FileModel for Diagram {
    const MAX_LEN: u64 = 64 * 1024 * 1024;
    const EXTENSION: &'static str = crate::EXTENSION;
}
