//! A diagram as a document: its elements, back to front, and the changes
//! that edit it (each returning its inverse, for undo). No drawing here, and
//! no file format: that's `format`'s.
use bedtime_document::DocumentModel;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A point on the canvas. X grows rightwards, Y downwards.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A box on the canvas: its top left corner and size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
        Self::new(
            self.x - by,
            self.y - by,
            self.w + 2.0 * by,
            self.h + 2.0 * by,
        )
    }

    /// Whether the point is in it, edges included. Nothing is in a box of
    /// negative size.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.right()
            && point.y >= self.y
            && point.y <= self.bottom()
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

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rectangle(Rect),
    Ellipse(Rect),
    /// A rhombus touching the middle of each side of its box.
    Diamond(Rect),
    /// Straight segments through the points; an arrow is a line with a head.
    Line {
        points: Vec<Point>,
        start: Head,
        end: Head,
        /// The element its first point is bound to: the point stays on that
        /// element's outline, wherever it goes.
        from: Option<ElementId>,
        /// The element its last point is bound to.
        to: Option<ElementId>,
        /// Where on the element its first point is bound to it sits.
        from_side: Side,
        to_side: Side,
    },
    /// A stroke drawn by hand.
    Freehand {
        points: Vec<Point>,
    },
    /// Text from its top left corner, `size` high.
    Text {
        at: Point,
        text: String,
        size: f32,
    },
    /// Some elements together: those that name it as their `group`, which
    /// come right after it, front of it. Its box is theirs, `padding` out
    /// from them all round (and room for its label above, if it has one):
    /// worked out from them (see `edit::with_bindings`), not kept in the
    /// file. Lines bind to it as to a box. It's drawn only with a label: a
    /// frame, and the label at its top left.
    Group {
        padding: f32,
        around: Rect,
    },
}

/// Text's size unless it says.
pub const TEXT_SIZE: f32 = 20.0;
/// A stroke's width unless it says.
pub const STROKE_WIDTH: f32 = 2.0;
/// The room between a group and what's in it, unless it says.
pub const GROUP_PADDING: f32 = 16.0;
/// How high a group's label is.
pub const GROUP_LABEL_SIZE: f32 = 16.0;
/// The room above what's in a group for its label, when it has one.
pub const GROUP_LABEL_ROOM: f32 = 28.0;

/// An element while the diagram is open: a handle for the editor, given to
/// it when it's read or made, and never reused. (The file names elements by
/// their `name`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementId(pub u64);

/// One thing on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub id: ElementId,
    /// What the file calls it (its `id` there), unique in the diagram: as
    /// written, or `kind-N` (`rect-3`, `arrow-7`) for one made in the
    /// editor. Empty until it's in a diagram, which names it.
    pub name: String,
    pub shape: Shape,
    /// What's written in the middle of a box (wrapped to its width) or
    /// halfway along a line; nothing for none. (Text on its own is a
    /// `Shape::Text`.)
    pub label: String,
    /// Its outline, line or text.
    pub stroke: Color,
    /// What fills a closed shape, as a tint.
    pub fill: Option<Color>,
    /// The stroke's width.
    pub width: f32,
    pub dash: Dash,
    /// A rectangle's corners are rounded.
    pub rounded: bool,
    /// The group it's in: elements of one group are selected together.
    pub group: Option<ElementId>,
}

impl Element {
    pub fn new(id: ElementId, shape: Shape) -> Self {
        Self {
            id,
            name: String::new(),
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

    pub fn is_group(&self) -> bool {
        matches!(self.shape, Shape::Group { .. })
    }

    /// The group it moves with: itself, for a group.
    pub fn unit(&self) -> Option<ElementId> {
        if self.is_group() {
            Some(self.id)
        } else {
            self.group
        }
    }

    /// Whether it can have a label: a box, a line or a group.
    pub fn takes_label(&self) -> bool {
        self.is_box() || matches!(self.shape, Shape::Line { .. } | Shape::Group { .. })
    }

    /// What it's called by kind, in names made for it (`rect-3`).
    pub fn kind(&self) -> &'static str {
        match &self.shape {
            Shape::Rectangle(_) => "rect",
            Shape::Ellipse(_) => "ellipse",
            Shape::Diamond(_) => "diamond",
            Shape::Line { start, end, .. } if *start == Head::Arrow || *end == Head::Arrow => {
                "arrow"
            }
            Shape::Line { .. } => "line",
            Shape::Freehand { .. } => "freehand",
            Shape::Text { .. } => "text",
            Shape::Group { .. } => "group",
        }
    }

    /// The box around it, not counting its stroke's width. Text is measured
    /// roughly (the canvas has the fonts): enough to place and fit it.
    pub fn bounds(&self) -> Rect {
        match &self.shape {
            Shape::Rectangle(rect)
            | Shape::Ellipse(rect)
            | Shape::Diamond(rect)
            | Shape::Group { around: rect, .. } => *rect,
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
    Insert {
        index: usize,
        element: Element,
    },
    Remove {
        id: ElementId,
    },
    /// Swap the element of this id for this one, where it is.
    Replace {
        element: Element,
    },
}

/// Whether a name is one the editor makes: `kind-N`.
pub fn is_made_name(name: &str) -> bool {
    name.rsplit_once('-').is_some_and(|(kind, number)| {
        [
            "rect", "ellipse", "diamond", "arrow", "line", "freehand", "text", "group",
        ]
        .contains(&kind)
            && !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

impl Diagram {
    /// A diagram of these elements, those with no name named (see
    /// `Element::name`); an id or a name used twice is refused.
    pub fn new(elements: Vec<Element>) -> Result<Self, String> {
        let mut ids: Vec<ElementId> = elements.iter().map(|element| element.id).collect();
        ids.sort_unstable();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(format!("Two elements have the id {}", pair[0].0));
        }
        let mut names: Vec<&str> = elements
            .iter()
            .map(|element| element.name.as_str())
            .filter(|name| !name.is_empty())
            .collect();
        names.sort_unstable();
        if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(format!("Two elements are called \"{}\"", pair[0]));
        }
        let mut diagram = Self {
            elements: Arc::new(elements),
        };
        for index in 0..diagram.elements.len() {
            if diagram.elements[index].name.is_empty() {
                let name = diagram.name_for(&diagram.elements[index]);
                Arc::make_mut(&mut diagram.elements)[index].name = name;
            }
        }
        Ok(diagram)
    }

    /// Whether it's the very same as `other`, not just equal: a cheap test
    /// that nothing has changed.
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.elements, &other.elements)
    }

    /// The element of a name.
    pub fn named(&self, name: &str) -> Option<&Element> {
        self.elements.iter().find(|element| element.name == name)
    }

    /// A name for a new element: its kind, and a number above any of that
    /// kind's here (`rect-4`).
    pub fn name_for(&self, element: &Element) -> String {
        let kind = element.kind();
        let highest = self
            .elements
            .iter()
            .filter_map(|other| {
                let (prefix, number) = other.name.rsplit_once('-')?;
                (prefix == kind).then(|| number.parse::<u64>().ok())?
            })
            .max()
            .unwrap_or(0);
        format!("{kind}-{}", highest + 1)
    }

    /// The elements of a group, back to front.
    pub fn members(&self, group: ElementId) -> impl Iterator<Item = &Element> {
        self.elements
            .iter()
            .filter(move |element| element.group == Some(group))
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
            // One with no name is named.
            Change::Insert { index, element } => {
                if self.get(element.id).is_some() {
                    return Err(format!("An element already has the id {}", element.id.0));
                }
                if *index > self.elements.len() {
                    return Err(format!("There's no place {index} to add an element at"));
                }
                let mut element = element.clone();
                if element.name.is_empty() {
                    element.name = self.name_for(&element);
                } else if self.named(&element.name).is_some() {
                    return Err(format!("An element is already called \"{}\"", element.name));
                }
                let id = element.id;
                Arc::make_mut(&mut self.elements).insert(*index, element);
                Ok(Change::Remove { id })
            }
            Change::Remove { id } => {
                let index = self.index_of(*id)?;
                let element = Arc::make_mut(&mut self.elements).remove(index);
                Ok(Change::Insert { index, element })
            }
            Change::Replace { element } => {
                let index = self.index_of(element.id)?;
                if self
                    .named(&element.name)
                    .is_some_and(|other| other.id != element.id)
                    || element.name.is_empty()
                {
                    return Err(format!("\"{}\" can't be its name", element.name));
                }
                let previous = std::mem::replace(
                    &mut Arc::make_mut(&mut self.elements)[index],
                    element.clone(),
                );
                Ok(Change::Replace { element: previous })
            }
        }
    }
}
