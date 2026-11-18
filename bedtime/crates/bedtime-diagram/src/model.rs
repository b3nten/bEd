//! A diagram as a document: its elements, back to front, the changes that
//! edit it (each returning its inverse, for undo), and the `.diagram` file
//! format: RON, one element per line, so diffs read well. No drawing here.
use bedtime_document::{ByteModel, DocumentModel};
use bedtime_filesystem::FileModel;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The file format's version: files of a later one aren't opened.
pub const VERSION: u32 = 1;

/// A point on the canvas. X grows rightwards, Y downwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "(f32, f32)", into = "(f32, f32)")]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

impl From<Point> for (f32, f32) {
    fn from(point: Point) -> Self {
        (point.x, point.y)
    }
}

/// A box on the canvas: its top left corner and size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// The smallest box holding the points; `None` for no points.
    pub fn around(points: impl IntoIterator<Item = Point>) -> Option<Self> {
        let mut points = points.into_iter();
        let first = points.next()?;
        let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.x, first.y);
        for point in points {
            left = left.min(point.x);
            top = top.min(point.y);
            right = right.max(point.x);
            bottom = bottom.max(point.y);
        }
        Some(Self::new(left, top, right - left, bottom - top))
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// With its width and height positive: the same box, whichever corner
    /// it was dragged from.
    pub fn normalized(&self) -> Self {
        Self::new(
            self.x.min(self.right()),
            self.y.min(self.bottom()),
            self.w.abs(),
            self.h.abs(),
        )
    }

    /// Grown by `by` on every side (shrunk, if negative).
    pub fn expand(&self, by: f32) -> Self {
        Self::new(self.x - by, self.y - by, self.w + 2.0 * by, self.h + 2.0 * by)
    }

    /// Whether the point is in it, edges included. Nothing is in a box of
    /// negative size.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x && point.x <= self.right() && point.y >= self.y && point.y <= self.bottom()
    }

    /// Whether all of `other` is in it.
    pub fn encloses(&self, other: &Self) -> bool {
        self.contains(Point::new(other.x, other.y))
            && self.contains(Point::new(other.right(), other.bottom()))
    }

    /// The smallest box holding both.
    pub fn union(&self, other: &Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }
}

/// A color by name, taken from the theme when drawn: a diagram reads in any
/// theme, light or dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Color {
    /// The theme's text color.
    #[default]
    Ink,
    Muted,
    Accent,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
}

impl Color {
    pub const ALL: [Self; 9] = [
        Self::Ink,
        Self::Muted,
        Self::Accent,
        Self::Red,
        Self::Green,
        Self::Yellow,
        Self::Blue,
        Self::Magenta,
        Self::Cyan,
    ];
}

/// How a stroke is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dash {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// What ends a line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Head {
    #[default]
    None,
    Arrow,
}

/// Where a line's bound end sits on what it's bound to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Wherever faces the way the line comes from: it goes around the
    /// outline as things move.
    #[default]
    Auto,
    /// The middle of that side, whatever moves.
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Rectangle(Rect),
    Ellipse(Rect),
    /// A rhombus touching the middle of each side of its box.
    Diamond(Rect),
    /// Straight segments through the points; an arrow is a line with a head.
    Line {
        points: Vec<Point>,
        #[serde(default, skip_serializing_if = "is_default")]
        start: Head,
        #[serde(default, skip_serializing_if = "is_default")]
        end: Head,
        /// The element its first point is bound to: the point stays on that
        /// element's outline, wherever it goes.
        #[serde(default, skip_serializing_if = "is_default")]
        from: Option<ElementId>,
        /// The element its last point is bound to.
        #[serde(default, skip_serializing_if = "is_default")]
        to: Option<ElementId>,
        /// Where on the element its first point is bound to it sits.
        #[serde(default, skip_serializing_if = "is_default")]
        from_side: Side,
        #[serde(default, skip_serializing_if = "is_default")]
        to_side: Side,
    },
    /// A stroke drawn by hand.
    Freehand { points: Vec<Point> },
    /// Text from its top left corner, `size` high.
    Text {
        at: Point,
        text: String,
        #[serde(default = "text_size")]
        size: f32,
    },
}

/// Text's size unless it says.
pub const TEXT_SIZE: f32 = 20.0;
/// A stroke's width unless it says.
pub const STROKE_WIDTH: f32 = 2.0;

fn text_size() -> f32 {
    TEXT_SIZE
}

fn stroke_width() -> f32 {
    STROKE_WIDTH
}

fn is_stroke_width(width: &f32) -> bool {
    *width == STROKE_WIDTH
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// An element's name within its diagram: kept for good, never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ElementId(pub u64);

/// One thing on the canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: ElementId,
    pub shape: Shape,
    /// What's written in the middle of a box (wrapped to its width) or
    /// halfway along a line; nothing for none. (Text on its own is a
    /// `Shape::Text`.)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Its outline, line or text.
    #[serde(default, skip_serializing_if = "is_default")]
    pub stroke: Color,
    /// What fills a closed shape, as a tint.
    #[serde(default, skip_serializing_if = "is_default")]
    pub fill: Option<Color>,
    /// The stroke's width.
    #[serde(default = "stroke_width", skip_serializing_if = "is_stroke_width")]
    pub width: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub dash: Dash,
    /// A rectangle's corners are rounded.
    #[serde(default, skip_serializing_if = "is_default")]
    pub rounded: bool,
    /// The group it's in: elements of one group are selected together.
    #[serde(default, skip_serializing_if = "is_default")]
    pub group: Option<u64>,
}

impl Element {
    pub fn new(id: ElementId, shape: Shape) -> Self {
        Self {
            id,
            shape,
            label: String::new(),
            stroke: Color::Ink,
            fill: None,
            width: STROKE_WIDTH,
            dash: Dash::Solid,
            rounded: false,
            group: None,
        }
    }

    /// Whether it's a box: something with an inside, that takes a label.
    pub fn is_box(&self) -> bool {
        matches!(
            self.shape,
            Shape::Rectangle(_) | Shape::Ellipse(_) | Shape::Diamond(_)
        )
    }

    /// Whether it can have a label: a box, or a line.
    pub fn takes_label(&self) -> bool {
        self.is_box() || matches!(self.shape, Shape::Line { .. })
    }

    /// The box around it, not counting its stroke's width. Text is measured
    /// roughly (the canvas has the fonts): enough to place and fit it.
    pub fn bounds(&self) -> Rect {
        match &self.shape {
            Shape::Rectangle(rect) | Shape::Ellipse(rect) | Shape::Diamond(rect) => *rect,
            Shape::Line { points, .. } | Shape::Freehand { points } => {
                Rect::around(points.iter().copied()).unwrap_or_default()
            }
            Shape::Text { at, text, size } => {
                let lines = text.lines().count().max(1) as f32;
                let widest = text
                    .lines()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(0) as f32;
                Rect::new(at.x, at.y, widest * size * 0.6, lines * size * 1.3)
            }
        }
    }
}

/// A diagram: its elements, back to front. Cheap to clone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diagram {
    elements: Arc<Vec<Element>>,
}

/// An edit to a diagram.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Add an element at `index`, counting from the back.
    Insert { index: usize, element: Element },
    Remove { id: ElementId },
    /// Swap the element of this id for this one, where it is.
    Replace { element: Element },
}

impl Diagram {
    /// A diagram of these elements; an id used twice is refused.
    pub fn new(elements: Vec<Element>) -> Result<Self, String> {
        let mut ids: Vec<ElementId> = elements.iter().map(|element| element.id).collect();
        ids.sort_unstable();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(format!("Two elements have the id {}", pair[0].0));
        }
        Ok(Self {
            elements: Arc::new(elements),
        })
    }

    /// Back to front.
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn get(&self, id: ElementId) -> Option<&Element> {
        self.elements.iter().find(|element| element.id == id)
    }

    fn index_of(&self, id: ElementId) -> Result<usize, String> {
        self.elements
            .iter()
            .position(|element| element.id == id)
            .ok_or_else(|| format!("No element has the id {}", id.0))
    }

    /// An id for a new element: above every one here.
    pub fn next_id(&self) -> ElementId {
        ElementId(
            self.elements
                .iter()
                .map(|element| element.id.0)
                .max()
                .map_or(1, |highest| highest + 1),
        )
    }

    /// A name for a new group: above every one here.
    pub fn next_group(&self) -> u64 {
        self.elements
            .iter()
            .filter_map(|element| element.group)
            .max()
            .map_or(1, |highest| highest + 1)
    }

    /// The box around everything; `None` when empty.
    pub fn bounds(&self) -> Option<Rect> {
        self.elements
            .iter()
            .map(Element::bounds)
            .reduce(|all, bounds| all.union(&bounds))
    }
}

impl DocumentModel for Diagram {
    type Change = Change;
    type OpenState = ();

    fn apply(&mut self, change: &Change) -> Result<Change, String> {
        match change {
            Change::Insert { index, element } => {
                if self.get(element.id).is_some() {
                    return Err(format!("An element already has the id {}", element.id.0));
                }
                if *index > self.elements.len() {
                    return Err(format!("There's no place {index} to add an element at"));
                }
                Arc::make_mut(&mut self.elements).insert(*index, element.clone());
                Ok(Change::Remove { id: element.id })
            }
            Change::Remove { id } => {
                let index = self.index_of(*id)?;
                let element = Arc::make_mut(&mut self.elements).remove(index);
                Ok(Change::Insert { index, element })
            }
            Change::Replace { element } => {
                let index = self.index_of(element.id)?;
                let previous =
                    std::mem::replace(&mut Arc::make_mut(&mut self.elements)[index], element.clone());
                Ok(Change::Replace { element: previous })
            }
        }
    }
}

/// The file, as RON reads it.
#[derive(Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    elements: Vec<Element>,
}

/// How the files are read and written: a shape's box is written in the
/// shape's own brackets (`Rectangle(x: 0, …)`).
fn ron_options() -> ron::Options {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::UNWRAP_VARIANT_NEWTYPES)
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
        Self::new(file.elements)
    }

    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        // One element per line, whatever it holds.
        let line = ron::ser::PrettyConfig::new()
            .compact_structs(true)
            .compact_arrays(true)
            .separate_tuple_members(false);
        writeln!(out, "(")?;
        writeln!(out, "    version: {VERSION},")?;
        writeln!(out, "    elements: [")?;
        for element in self.elements.iter() {
            let element = ron_options()
                .to_string_pretty(element, line.clone())
                .map_err(std::io::Error::other)?;
            writeln!(out, "        {element},")?;
        }
        writeln!(out, "    ],")?;
        writeln!(out, ")")
    }
}

impl FileModel for Diagram {
    const MAX_LEN: u64 = 64 * 1024 * 1024;
}
