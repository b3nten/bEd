//! The arithmetic of editing, with no drawing: what's under a point, what a
//! box encloses, elements moved, resized and made by dragging, and lines
//! kept on the outlines of the elements they're bound to.
use crate::model::{
    Change, Color, Dash, Diagram, Element, ElementId, Head, Point, Rect, STROKE_WIDTH, Shape, Side,
    TEXT_SIZE,
};
use bedtime_document::{ByteModel, DocumentModel};
use std::collections::HashMap;

/// What dragging on the canvas does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tool {
    /// Select, move and resize.
    #[default]
    Select,
    Rectangle,
    Ellipse,
    Diamond,
    Line,
    Arrow,
    /// Click to write text.
    Text,
    /// Draw a stroke by hand.
    Pen,
}

impl Tool {
    pub const ALL: [Self; 8] = [
        Self::Select,
        Self::Rectangle,
        Self::Ellipse,
        Self::Diamond,
        Self::Line,
        Self::Arrow,
        Self::Text,
        Self::Pen,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Diamond => "Diamond",
            Self::Line => "Line",
            Self::Arrow => "Arrow",
            Self::Text => "Text",
            Self::Pen => "Pen",
        }
    }

    /// The key that picks it.
    pub fn key(self) -> char {
        match self {
            Self::Select => 'V',
            Self::Rectangle => 'R',
            Self::Ellipse => 'O',
            Self::Diamond => 'D',
            Self::Line => 'L',
            Self::Arrow => 'A',
            Self::Text => 'T',
            Self::Pen => 'P',
        }
    }
}

/// A drag shorter than this (both ways) makes nothing: it was a click.
pub const SMALLEST: f32 = 4.0;

/// The element `tool` makes by dragging from `from` to `to`, if the drag is
/// long enough. `constrain` makes boxes square and lines turn in steps of 15°.
pub fn create(tool: Tool, id: ElementId, from: Point, to: Point, constrain: bool) -> Option<Element> {
    let (mut dx, mut dy) = (to.x - from.x, to.y - from.y);
    if dx.abs() < SMALLEST && dy.abs() < SMALLEST {
        return None;
    }
    let shape = match tool {
        // Text is written where it's clicked, not dragged out, and a
        // stroke is more than where it began and ended (see `stroke`).
        Tool::Select | Tool::Text | Tool::Pen => return None,
        Tool::Rectangle | Tool::Ellipse | Tool::Diamond => {
            if constrain {
                let side = dx.abs().max(dy.abs());
                dx = side.copysign(dx);
                dy = side.copysign(dy);
            }
            let rect = Rect::new(from.x, from.y, dx, dy).normalized();
            match tool {
                Tool::Rectangle => Shape::Rectangle(rect),
                Tool::Ellipse => Shape::Ellipse(rect),
                _ => Shape::Diamond(rect),
            }
        }
        Tool::Line | Tool::Arrow => {
            if constrain {
                let step = std::f32::consts::PI / 12.0;
                let angle = (dy.atan2(dx) / step).round() * step;
                let length = dx.hypot(dy);
                dx = length * angle.cos();
                dy = length * angle.sin();
            }
            Shape::Line {
                points: vec![from, Point::new(from.x + dx, from.y + dy)],
                start: Head::None,
                end: if tool == Tool::Arrow {
                    Head::Arrow
                } else {
                    Head::None
                },
                from: None,
                to: None,
                from_side: Side::Auto,
                to_side: Side::Auto,
            }
        }
    };
    Some(Element::new(id, shape))
}

/// The element moved by a distance.
pub fn translate(element: &Element, dx: f32, dy: f32) -> Element {
    let shift = |point: &Point| Point::new(point.x + dx, point.y + dy);
    let mut moved = element.clone();
    match &mut moved.shape {
        Shape::Rectangle(rect) | Shape::Ellipse(rect) | Shape::Diamond(rect) => {
            rect.x += dx;
            rect.y += dy;
        }
        Shape::Line { points, .. } | Shape::Freehand { points } => {
            points.iter_mut().for_each(|point| *point = shift(point));
        }
        Shape::Text { at, .. } => *at = shift(at),
    }
    moved
}

/// The element as it is when the box `from` (around it, or around a selection
/// it's part of) is stretched to `to`. `to` may have a negative width or
/// height: dragged past its other side, which mirrors what's in it. Text
/// moves with its corner and keeps its size.
pub fn remap(element: &Element, from: Rect, to: Rect) -> Element {
    // A box with no width (a vertical line's) can't be scaled: it moves.
    let scale = |from: f32, to: f32| if from.abs() > 1e-3 { to / from } else { 1.0 };
    let (sx, sy) = (scale(from.w, to.w), scale(from.h, to.h));
    let place = |point: &Point| {
        Point::new(
            to.x + (point.x - from.x) * sx,
            to.y + (point.y - from.y) * sy,
        )
    };
    let mut resized = element.clone();
    match &mut resized.shape {
        Shape::Rectangle(rect) | Shape::Ellipse(rect) | Shape::Diamond(rect) => {
            let corner = place(&Point::new(rect.x, rect.y));
            let opposite = place(&Point::new(rect.right(), rect.bottom()));
            *rect =
                Rect::new(corner.x, corner.y, opposite.x - corner.x, opposite.y - corner.y)
                    .normalized();
        }
        Shape::Line { points, .. } | Shape::Freehand { points } => {
            points.iter_mut().for_each(|point| *point = place(point));
        }
        Shape::Text { at, .. } => *at = place(at),
    }
    resized
}

/// A line with one of its points moved; anything else as it was.
pub fn with_point(element: &Element, index: usize, to: Point) -> Element {
    let mut changed = element.clone();
    if let Shape::Line { points, .. } = &mut changed.shape
        && let Some(point) = points.get_mut(index)
    {
        *point = to;
    }
    changed
}

/// A line with a point added at `index` (between the points on either side
/// of it); anything else as it was.
pub fn with_point_added(element: &Element, index: usize, at: Point) -> Element {
    let mut changed = element.clone();
    if let Shape::Line { points, .. } = &mut changed.shape
        && (1..points.len()).contains(&index)
    {
        points.insert(index, at);
    }
    changed
}

/// A line without one of the points it bends at; `None` for its ends, which
/// it can't do without.
pub fn without_point(element: &Element, index: usize) -> Option<Element> {
    let mut changed = element.clone();
    let Shape::Line { points, .. } = &mut changed.shape else {
        return None;
    };
    if index == 0 || index + 1 >= points.len() {
        return None;
    }
    points.remove(index);
    Some(changed)
}

/// Where among a line's points a point added at `at` goes: into the segment
/// nearest it.
pub fn segment_at(points: &[Point], at: Point) -> Option<usize> {
    (1..points.len()).min_by(|a, b| {
        let distance = |index: &usize| distance_to_segment(at, points[index - 1], points[*index]);
        distance(a).total_cmp(&distance(b))
    })
}

/// A line going the other way: its heads, and what it's bound to, change
/// ends with it. Anything else as it was.
pub fn reversed(element: &Element) -> Element {
    let mut changed = element.clone();
    if let Shape::Line {
        points,
        start,
        end,
        from,
        to,
        from_side,
        to_side,
    } = &mut changed.shape
    {
        points.reverse();
        std::mem::swap(start, end);
        std::mem::swap(from, to);
        std::mem::swap(from_side, to_side);
    }
    changed
}

/// What a line's ends have on them: `None` for anything else.
pub fn heads(element: &Element) -> Option<(Head, Head)> {
    match &element.shape {
        Shape::Line { start, end, .. } => Some((*start, *end)),
        _ => None,
    }
}

fn distance_to_segment(at: Point, a: Point, b: Point) -> f32 {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let length = abx * abx + aby * aby;
    let along = if length > 0.0 {
        (((at.x - a.x) * abx + (at.y - a.y) * aby) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (at.x - (a.x + abx * along)).hypot(at.y - (a.y + aby * along))
}

/// The box around an element, for picking it. `Element::bounds` only guesses
/// at text's; the canvas, which has the fonts, measures it.
pub type Extent<'a> = &'a dyn Fn(&Element) -> Rect;

/// Whether a point touches the element, give or take `reach`: its stroke, or
/// anywhere in it if it's filled (an outline is picked by its outline, so
/// what's inside it can be).
pub fn hit(element: &Element, at: Point, reach: f32, extent: Extent<'_>) -> bool {
    let reach = reach + element.width / 2.0;
    let filled = element.fill.is_some();
    match &element.shape {
        Shape::Rectangle(rect) => {
            rect.expand(reach).contains(at) && (filled || !rect.expand(-reach).contains(at))
        }
        Shape::Ellipse(rect) | Shape::Diamond(rect) => {
            let center = rect.center();
            let (dx, dy) = ((at.x - center.x).abs(), (at.y - center.y).abs());
            let round = matches!(element.shape, Shape::Ellipse(_));
            let within = |grow: f32| {
                let (rx, ry) = (rect.w / 2.0 + grow, rect.h / 2.0 + grow);
                rx > 0.0
                    && ry > 0.0
                    && if round {
                        (dx / rx).powi(2) + (dy / ry).powi(2) <= 1.0
                    } else {
                        dx / rx + dy / ry <= 1.0
                    }
            };
            within(reach) && (filled || !within(-reach))
        }
        Shape::Line { points, .. } | Shape::Freehand { points } => match points.as_slice() {
            [] => false,
            [only] => distance_to_segment(at, *only, *only) <= reach,
            points => points
                .windows(2)
                .any(|pair| distance_to_segment(at, pair[0], pair[1]) <= reach),
        },
        Shape::Text { .. } => extent(element).expand(reach).contains(at),
    }
}

/// The frontmost element a point touches.
pub fn topmost_at(
    diagram: &Diagram,
    at: Point,
    reach: f32,
    extent: Extent<'_>,
) -> Option<ElementId> {
    diagram
        .elements()
        .iter()
        .rev()
        .find(|element| hit(element, at, reach, extent))
        .map(|element| element.id)
}

/// The frontmost element with text to write at a point: text itself, a box
/// the point is in (filled or not: its label goes in its middle), or a line
/// within `reach`.
pub fn writable_at(
    diagram: &Diagram,
    at: Point,
    reach: f32,
    extent: Extent<'_>,
) -> Option<ElementId> {
    diagram
        .elements()
        .iter()
        .rev()
        .find(|element| match &element.shape {
            Shape::Text { .. } => extent(element).contains(at),
            _ if element.is_box() => {
                let mut solid = (*element).clone();
                solid.fill.get_or_insert_default();
                hit(&solid, at, 0.0, extent)
            }
            Shape::Line { .. } => hit(element, at, reach, extent),
            _ => false,
        })
        .map(|element| element.id)
}

/// The elements wholly inside a box, back to front.
pub fn enclosed(diagram: &Diagram, area: Rect, extent: Extent<'_>) -> Vec<ElementId> {
    let area = area.normalized();
    diagram
        .elements()
        .iter()
        .filter(|element| area.encloses(&extent(element)))
        .map(|element| element.id)
        .collect()
}

/// A place to drag a box by: a corner, or the middle of a side. `x` and `y`
/// are -1 (left, top), 0 (middle) or 1 (right, bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub x: i8,
    pub y: i8,
}

impl Handle {
    pub const ALL: [Self; 8] = [
        Self { x: -1, y: -1 },
        Self { x: 1, y: -1 },
        Self { x: 1, y: 1 },
        Self { x: -1, y: 1 },
        Self { x: 0, y: -1 },
        Self { x: 1, y: 0 },
        Self { x: 0, y: 1 },
        Self { x: -1, y: 0 },
    ];

    pub fn is_corner(self) -> bool {
        self.x != 0 && self.y != 0
    }

    /// The box with this handle dragged to a point: the sides it's on follow,
    /// the others stay. Past the other side, the size turns negative (see
    /// `remap`).
    pub fn drag(self, rect: Rect, to: Point) -> Rect {
        let (mut left, mut top, mut right, mut bottom) =
            (rect.x, rect.y, rect.right(), rect.bottom());
        match self.x {
            -1 => left = to.x,
            1 => right = to.x,
            _ => {}
        }
        match self.y {
            -1 => top = to.y,
            1 => bottom = to.y,
            _ => {}
        }
        Rect::new(left, top, right - left, bottom - top)
    }
}

// ── Binding ──────────────────────────────────────────────────────────────────
//
// A line's end bound to an element sits on that element's outline, a little
// off it: in the middle of a side, if it's pinned to one, else on the way from
// the element's middle to where the line comes from. An end dropped near a
// side is pinned to it; dropped in the element's middle, it isn't. The points
// are kept in the line, and settled again whenever something they depend on
// changes, in the same step of undo.

/// How near (on the canvas) a line's end must be to an element's box to bind
/// to it.
pub const BIND_REACH: f32 = 8.0;
/// The gap between a bound end and the outline it's on.
pub const BIND_GAP: f32 = 4.0;

/// The frontmost element a line's end at this point would bind to: a box, or
/// text, whose box the point is in or near. `not` is the line itself.
pub fn bindable_at(
    diagram: &Diagram,
    at: Point,
    extent: Extent<'_>,
    not: Option<ElementId>,
) -> Option<ElementId> {
    diagram
        .elements()
        .iter()
        .rev()
        .filter(|element| Some(element.id) != not)
        .filter(|element| element.is_box() || matches!(element.shape, Shape::Text { .. }))
        .find(|element| extent(element).expand(BIND_REACH).contains(at))
        .map(|element| element.id)
}

/// The side of an element a line's end dropped at this point is pinned to:
/// the nearest, unless the point is in the element's middle (the inner half
/// of its box, each way), which pins it to none.
pub fn side_at(target: &Element, at: Point, extent: Extent<'_>) -> Side {
    let rect = extent(target);
    let center = rect.center();
    let across = (at.x - center.x) / (rect.w / 2.0).max(0.5);
    let down = (at.y - center.y) / (rect.h / 2.0).max(0.5);
    if across.abs().max(down.abs()) < 0.5 {
        Side::Auto
    } else if across.abs() > down.abs() {
        if across > 0.0 { Side::Right } else { Side::Left }
    } else if down > 0.0 {
        Side::Bottom
    } else {
        Side::Top
    }
}

/// What a line's end at this point would bind to (see `bindable_at`), and the
/// side it would be pinned to there.
pub fn binding_at(
    diagram: &Diagram,
    at: Point,
    extent: Extent<'_>,
    not: Option<ElementId>,
) -> Option<(ElementId, Side)> {
    let id = bindable_at(diagram, at, extent, not)?;
    Some((id, side_at(diagram.get(id)?, at, extent)))
}

/// Bind a line's point to an element (pinned to a side, or not), or to
/// nothing, if it's the line's first or last. Both ends aren't bound to the
/// same element: the second isn't.
pub fn bind_end(line: &mut Element, index: usize, target: Option<(ElementId, Side)>) {
    let Shape::Line {
        points,
        from,
        to,
        from_side,
        to_side,
        ..
    } = &mut line.shape
    else {
        return;
    };
    let (end, side, other) = if index == 0 {
        (from, from_side, *to)
    } else if index + 1 == points.len() {
        (to, to_side, *from)
    } else {
        return;
    };
    let target = target.filter(|(id, _)| Some(*id) != other);
    *end = target.map(|(id, _)| id);
    *side = target.map_or(Side::Auto, |(_, side)| side);
}

/// Unbind a line's ends from the elements `keep` doesn't name (a line moved
/// without what it's bound to lets go of it).
pub fn unbind_unless(line: &mut Element, keep: impl Fn(ElementId) -> bool) {
    if let Shape::Line {
        from,
        to,
        from_side,
        to_side,
        ..
    } = &mut line.shape
    {
        *from = from.filter(|id| keep(*id));
        *to = to.filter(|id| keep(*id));
        if from.is_none() {
            *from_side = Side::Auto;
        }
        if to.is_none() {
            *to_side = Side::Auto;
        }
    }
}

/// Where a line bound to `target` meets it, coming from `toward`: on the
/// target's outline (text's is its box), on the way from its middle, and
/// `BIND_GAP` off it.
pub fn attach(target: &Element, toward: Point, extent: Extent<'_>) -> Point {
    let rect = extent(target);
    let center = rect.center();
    let (mut dx, mut dy) = (toward.x - center.x, toward.y - center.y);
    if dx.hypot(dy) < 1e-3 {
        (dx, dy) = (1.0, 0.0);
    }
    let (rx, ry) = ((rect.w / 2.0).max(0.5), (rect.h / 2.0).max(0.5));
    // How far along (dx, dy) the outline is.
    let along = match target.shape {
        Shape::Ellipse(_) => 1.0 / ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt(),
        Shape::Diamond(_) => 1.0 / (dx.abs() / rx + dy.abs() / ry),
        _ => (rx / dx.abs()).min(ry / dy.abs()),
    };
    let length = dx.hypot(dy);
    let off = (BIND_GAP + target.width / 2.0) / length;
    Point::new(
        center.x + dx * (along + off),
        center.y + dy * (along + off),
    )
}

/// Where a line pinned to a side of `target` meets it: the middle of that
/// side, `BIND_GAP` off it. `None` for no side.
pub fn attach_side(target: &Element, side: Side, extent: Extent<'_>) -> Option<Point> {
    let rect = extent(target);
    let center = rect.center();
    let gap = BIND_GAP + target.width / 2.0;
    Some(match side {
        Side::Auto => return None,
        Side::Top => Point::new(center.x, rect.y - gap),
        Side::Right => Point::new(rect.right() + gap, center.y),
        Side::Bottom => Point::new(center.x, rect.bottom() + gap),
        Side::Left => Point::new(rect.x - gap, center.y),
    })
}

/// A line with its bound ends where they belong in `diagram`, and unbound
/// from elements that aren't there. An end pinned to a side is in the middle
/// of it. One that isn't faces where the line comes from: the line's next
/// point, or, for a line straight between two elements, the other's pinned
/// end or else its middle. Anything but a line is returned as it is.
pub fn settle(line: &Element, diagram: &Diagram, extent: Extent<'_>) -> Element {
    let mut settled = line.clone();
    let Shape::Line {
        points,
        from,
        to,
        from_side,
        to_side,
        ..
    } = &mut settled.shape
    else {
        return settled;
    };
    let first = from.and_then(|id| diagram.get(id));
    let last = to.and_then(|id| diagram.get(id));
    if first.is_none() {
        (*from, *from_side) = (None, Side::Auto);
    }
    if last.is_none() {
        (*to, *to_side) = (None, Side::Auto);
    }
    let count = points.len();
    if count < 2 {
        return settled;
    }
    let pinned_first = first.and_then(|first| attach_side(first, *from_side, extent));
    let pinned_last = last.and_then(|last| attach_side(last, *to_side, extent));
    let middle = |element: &Element| extent(element).center();
    let straight = count == 2;
    // What each end faces.
    let toward_first = match (straight, pinned_last, last) {
        (true, Some(pinned), _) => pinned,
        (true, None, Some(last)) => middle(last),
        _ => points[1],
    };
    let toward_last = match (straight, pinned_first, first) {
        (true, Some(pinned), _) => pinned,
        (true, None, Some(first)) => middle(first),
        _ => points[count - 2],
    };
    if let Some(first) = first {
        points[0] = pinned_first.unwrap_or_else(|| attach(first, toward_first, extent));
    }
    if let Some(last) = last {
        points[count - 1] = pinned_last.unwrap_or_else(|| attach(last, toward_last, extent));
    }
    settled
}

/// The changes, and what follows from them for bound lines: a line that was
/// changed, or is bound to something that was, is settled again, and lets go
/// of what was removed.
pub fn with_bindings(diagram: &Diagram, changes: Vec<Change>, extent: Extent<'_>) -> Vec<Change> {
    let mut after = diagram.clone();
    let mut touched = Vec::new();
    for change in &changes {
        touched.push(match change {
            Change::Insert { element, .. } | Change::Replace { element } => element.id,
            Change::Remove { id } => *id,
        });
        // Changes that can't be made are the caller's to find out about.
        if after.apply(change).is_err() {
            return changes;
        }
    }
    let mut all = changes;
    for element in after.elements() {
        let Shape::Line { from, to, .. } = &element.shape else {
            continue;
        };
        let concerned = [Some(element.id), *from, *to]
            .into_iter()
            .flatten()
            .any(|id| touched.contains(&id));
        if concerned {
            let settled = settle(element, &after, extent);
            if settled != *element {
                all.push(Change::Replace { element: settled });
            }
        }
    }
    all
}

/// The point halfway along a line, by its length.
pub fn midpoint(points: &[Point]) -> Option<Point> {
    let (first, rest) = points.split_first()?;
    let length = |a: &Point, b: &Point| (b.x - a.x).hypot(b.y - a.y);
    let total: f32 = points.windows(2).map(|pair| length(&pair[0], &pair[1])).sum();
    let mut left = total / 2.0;
    let mut from = first;
    for to in rest {
        let step = length(from, to);
        if step >= left && step > 0.0 {
            let along = left / step;
            return Some(Point::new(
                from.x + (to.x - from.x) * along,
                from.y + (to.y - from.y) * along,
            ));
        }
        left -= step;
        from = to;
    }
    Some(*from)
}

// ── Style ────────────────────────────────────────────────────────────────────

/// One change of style, to whatever takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Restyle {
    /// Anything's outline, line or text.
    Stroke(Color),
    /// A box's fill.
    Fill(Option<Color>),
    /// A stroke's width: anything but text.
    Width(f32),
    /// Anything but text.
    Dash(Dash),
    /// A rectangle's corners.
    Rounded(bool),
    /// Text's size.
    Size(f32),
    /// What a line begins with. (Not a style of what's made next: the Line
    /// and Arrow tools say that.)
    Start(Head),
    /// What a line ends with.
    End(Head),
}

/// The element with a style changed, if it takes that style: a line has no
/// fill, text no stroke width.
pub fn restyle(element: &Element, change: Restyle) -> Element {
    let mut styled = element.clone();
    let is_text = matches!(element.shape, Shape::Text { .. });
    match change {
        Restyle::Stroke(color) => styled.stroke = color,
        Restyle::Fill(fill) if element.is_box() => styled.fill = fill,
        Restyle::Width(width) if !is_text => styled.width = width,
        Restyle::Dash(dash) if !is_text => styled.dash = dash,
        Restyle::Rounded(rounded) if matches!(element.shape, Shape::Rectangle(_)) => {
            styled.rounded = rounded;
        }
        Restyle::Size(to) => {
            if let Shape::Text { size, .. } = &mut styled.shape {
                *size = to;
            }
        }
        Restyle::Start(head) => {
            if let Shape::Line { start, .. } = &mut styled.shape {
                *start = head;
            }
        }
        Restyle::End(head) => {
            if let Shape::Line { end, .. } = &mut styled.shape {
                *end = head;
            }
        }
        _ => {}
    }
    styled
}

/// The style new elements are given: the last picked of each kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub stroke: Color,
    pub fill: Option<Color>,
    pub width: f32,
    pub dash: Dash,
    pub rounded: bool,
    pub size: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            stroke: Color::Ink,
            fill: None,
            width: STROKE_WIDTH,
            dash: Dash::Solid,
            rounded: false,
            size: TEXT_SIZE,
        }
    }
}

impl Style {
    /// An element's style, where it has one; the rest as here.
    pub fn of(self, element: &Element) -> Self {
        let is_text = matches!(element.shape, Shape::Text { .. });
        Self {
            stroke: element.stroke,
            fill: if element.is_box() { element.fill } else { self.fill },
            width: if is_text { self.width } else { element.width },
            dash: if is_text { self.dash } else { element.dash },
            rounded: if matches!(element.shape, Shape::Rectangle(_)) {
                element.rounded
            } else {
                self.rounded
            },
            size: match element.shape {
                Shape::Text { size, .. } => size,
                _ => self.size,
            },
        }
    }

    pub fn set(&mut self, change: Restyle) {
        match change {
            Restyle::Stroke(color) => self.stroke = color,
            Restyle::Fill(fill) => self.fill = fill,
            Restyle::Width(width) => self.width = width,
            Restyle::Dash(dash) => self.dash = dash,
            Restyle::Rounded(rounded) => self.rounded = rounded,
            Restyle::Size(size) => self.size = size,
            Restyle::Start(_) | Restyle::End(_) => {}
        }
    }

    /// The element in this style, as far as it takes it.
    pub fn dress(&self, element: &Element) -> Element {
        [
            Restyle::Stroke(self.stroke),
            Restyle::Fill(self.fill),
            Restyle::Width(self.width),
            Restyle::Dash(self.dash),
            Restyle::Rounded(self.rounded),
            Restyle::Size(self.size),
        ]
        .into_iter()
        .fold(element.clone(), |element, change| restyle(&element, change))
    }
}

/// How far a rounded rectangle's corners are rounded, on the canvas.
pub fn corner_radius(rect: &Rect) -> f32 {
    (rect.w.min(rect.h) * 0.25).min(16.0)
}

// ── Strokes drawn by hand ────────────────────────────────────────────────────
//
// The pointer gives far more points than a stroke needs. What's kept are the
// ones that matter (`simplify`), to a tenth of a unit; the curve through them
// is made again when it's drawn (`smooth`).

/// The points with those left out that are within `tolerance` of the line
/// through the ones kept (Ramer–Douglas–Peucker). The first and last stay.
pub fn simplify(points: &[Point], tolerance: f32) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    // Stretches still to look at: the furthest point of each from the line
    // between its ends is kept, if it's far enough, and splits it in two.
    let mut stretches = vec![(0, points.len() - 1)];
    while let Some((first, last)) = stretches.pop() {
        let furthest = (first + 1..last)
            .map(|index| {
                (
                    index,
                    distance_to_segment(points[index], points[first], points[last]),
                )
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((index, distance)) = furthest
            && distance > tolerance
        {
            keep[index] = true;
            stretches.push((first, index));
            stretches.push((index, last));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter(|(_, keep)| *keep)
        .map(|(point, _)| *point)
        .collect()
}

/// A stroke through the points the pointer went through: simplified to
/// within `tolerance`, kept to a tenth of a unit. `None` if that leaves less
/// than a line.
pub fn stroke(id: ElementId, points: &[Point], tolerance: f32) -> Option<Element> {
    let tenth = |value: f32| (value * 10.0).round() / 10.0;
    let mut points: Vec<Point> = simplify(points, tolerance)
        .into_iter()
        .map(|point| Point::new(tenth(point.x), tenth(point.y)))
        .collect();
    points.dedup();
    (points.len() >= 2).then(|| Element::new(id, Shape::Freehand { points }))
}

/// A smooth curve through the points (a Catmull–Rom spline), as points no
/// more than about `spacing` apart, and at most 16 to a stretch. It passes
/// through every point given.
pub fn smooth(points: &[Point], spacing: f32) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let at = |index: isize| points[index.clamp(0, points.len() as isize - 1) as usize];
    let mut curve = vec![points[0]];
    for index in 0..points.len() as isize - 1 {
        let (before, from, to, after) = (at(index - 1), at(index), at(index + 1), at(index + 2));
        let length = (to.x - from.x).hypot(to.y - from.y);
        let steps = ((length / spacing.max(0.1)).ceil() as usize).clamp(1, 16);
        let along = |a: f32, b: f32, c: f32, d: f32, t: f32| {
            0.5 * (2.0 * b
                + (c - a) * t
                + (2.0 * a - 5.0 * b + 4.0 * c - d) * t * t
                + (3.0 * b - a - 3.0 * c + d) * t * t * t)
        };
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            curve.push(if step == steps {
                to
            } else {
                Point::new(
                    along(before.x, from.x, to.x, after.x, t),
                    along(before.y, from.y, to.y, after.y, t),
                )
            });
        }
    }
    curve
}

// ── Copies ───────────────────────────────────────────────────────────────────

/// Copies of some elements for `diagram`, moved by a distance: with new ids,
/// their groups new groups, and their lines bound to the copies of what the
/// originals were bound to (and to nothing, where that wasn't copied).
pub fn copies(elements: &[&Element], diagram: &Diagram, dx: f32, dy: f32) -> Vec<Element> {
    let first = diagram.next_id().0;
    let ids: HashMap<ElementId, ElementId> = elements
        .iter()
        .zip(first..)
        .map(|(element, id)| (element.id, ElementId(id)))
        .collect();
    let mut next_group = diagram.next_group();
    let mut groups = HashMap::new();
    elements
        .iter()
        .map(|element| {
            let mut copy = translate(element, dx, dy);
            copy.id = ids[&element.id];
            copy.group = element.group.map(|group| {
                *groups.entry(group).or_insert_with(|| {
                    next_group += 1;
                    next_group - 1
                })
            });
            if let Shape::Line { from, to, .. } = &mut copy.shape {
                *from = from.and_then(|id| ids.get(&id).copied());
                *to = to.and_then(|id| ids.get(&id).copied());
            }
            // Sides of what it's no longer bound to are forgotten.
            unbind_unless(&mut copy, |_| true);
            copy
        })
        .collect()
}

/// The changes that add elements in front of everything, in their order.
pub fn insertions(diagram: &Diagram, elements: Vec<Element>) -> Vec<Change> {
    let front = diagram.elements().len();
    elements
        .into_iter()
        .enumerate()
        .map(|(index, element)| Change::Insert {
            index: front + index,
            element,
        })
        .collect()
}

/// Elements as text for the clipboard: a diagram file of just them.
pub fn to_text(elements: &[&Element]) -> String {
    let Ok(diagram) = Diagram::new(elements.iter().map(|element| (*element).clone()).collect())
    else {
        return String::new();
    };
    let mut bytes = Vec::new();
    match diagram.encode(&mut bytes) {
        Ok(()) => String::from_utf8(bytes).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// The elements in text from the clipboard, if it's a diagram's.
pub fn from_text(text: &str) -> Option<Vec<Element>> {
    let diagram = Diagram::decode(text.as_bytes()).ok()?;
    (!diagram.is_empty()).then(|| diagram.elements().to_vec())
}

// ── Order ────────────────────────────────────────────────────────────────────

/// A way to move elements among the others, front to back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// In front of everything.
    Front,
    /// In front of the next element ahead.
    Forward,
    Backward,
    Back,
}

/// The changes that move some elements in the diagram's order, keeping their
/// own order; none if they're already there.
pub fn reorder(diagram: &Diagram, ids: &[ElementId], order: Order) -> Vec<Change> {
    let before: Vec<&Element> = diagram.elements().iter().collect();
    let moving = |element: &&Element| ids.contains(&element.id);
    let mut after = before.clone();
    match order {
        Order::Front | Order::Back => {
            let (moved, mut rest): (Vec<_>, Vec<_>) = before.iter().partition(|e| moving(e));
            after = if order == Order::Front {
                rest.extend(moved);
                rest
            } else {
                moved.into_iter().chain(rest).collect()
            };
        }
        // Each steps past the one ahead of it, unless that's moving too.
        Order::Forward => {
            for index in (0..after.len().saturating_sub(1)).rev() {
                if moving(&after[index]) && !moving(&after[index + 1]) {
                    after.swap(index, index + 1);
                }
            }
        }
        Order::Backward => {
            for index in 1..after.len() {
                if moving(&after[index]) && !moving(&after[index - 1]) {
                    after.swap(index, index - 1);
                }
            }
        }
    }
    if after.iter().map(|e| e.id).eq(before.iter().map(|e| e.id)) {
        return Vec::new();
    }
    // Taken out, then put back where they go, from the back forwards.
    let out = before
        .iter()
        .filter(|e| moving(e))
        .map(|element| Change::Remove { id: element.id });
    let back = after
        .iter()
        .enumerate()
        .filter(|(_, e)| moving(e))
        .map(|(index, element)| Change::Insert {
            index,
            element: (*element).clone(),
        });
    out.chain(back).collect()
}

// ── Groups ───────────────────────────────────────────────────────────────────

/// The ids, and those of everything grouped with any of them.
pub fn with_groups(diagram: &Diagram, ids: &[ElementId]) -> Vec<ElementId> {
    with_groups_but(diagram, ids, None)
}

/// The ids, and those of everything grouped with any of them, but for the
/// group `within`: its elements are picked one by one.
pub fn with_groups_but(diagram: &Diagram, ids: &[ElementId], within: Option<u64>) -> Vec<ElementId> {
    let groups: Vec<u64> = ids
        .iter()
        .filter_map(|id| diagram.get(*id)?.group)
        .filter(|group| Some(*group) != within)
        .collect();
    let mut all = ids.to_vec();
    if !groups.is_empty() {
        for element in diagram.elements() {
            if element.group.is_some_and(|group| groups.contains(&group))
                && !all.contains(&element.id)
            {
                all.push(element.id);
            }
        }
    }
    all
}

/// The changes that make one new group of some elements (two or more),
/// whatever groups they were in.
pub fn group(diagram: &Diagram, ids: &[ElementId]) -> Vec<Change> {
    if ids.len() < 2 {
        return Vec::new();
    }
    regroup(diagram, ids, Some(diagram.next_group()))
}

/// The changes that take some elements out of their groups.
pub fn ungroup(diagram: &Diagram, ids: &[ElementId]) -> Vec<Change> {
    regroup(diagram, ids, None)
}

fn regroup(diagram: &Diagram, ids: &[ElementId], group: Option<u64>) -> Vec<Change> {
    diagram
        .elements()
        .iter()
        .filter(|element| ids.contains(&element.id) && element.group != group)
        .map(|element| Change::Replace {
            element: Element {
                group,
                ..element.clone()
            },
        })
        .collect()
}

// ── Aligning ─────────────────────────────────────────────────────────────────

/// What of some elements to line up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Top,
    Middle,
    Bottom,
}

/// The selection as what moves together: each group, and each element in no
/// group (or in the group `within`, gone into), with the box around it.
fn units<'a>(
    diagram: &'a Diagram,
    ids: &[ElementId],
    within: Option<u64>,
    extent: Extent<'_>,
) -> Vec<(Rect, Vec<&'a Element>)> {
    let mut units: Vec<(Option<u64>, Rect, Vec<&Element>)> = Vec::new();
    for element in diagram.elements().iter().filter(|e| ids.contains(&e.id)) {
        let bounds = extent(element);
        let group = element.group.filter(|group| Some(*group) != within);
        match units
            .iter_mut()
            .find(|(other, ..)| other.is_some() && *other == group)
        {
            Some((_, all, members)) => {
                *all = all.union(&bounds);
                members.push(element);
            }
            None => units.push((group, bounds, vec![element])),
        }
    }
    units.into_iter().map(|(_, bounds, members)| (bounds, members)).collect()
}

/// How many things of a selection move on their own (see `align`).
pub fn unit_count(diagram: &Diagram, ids: &[ElementId], within: Option<u64>) -> usize {
    units(diagram, ids, within, &Element::bounds).len()
}

/// The changes that line some elements up: their left sides with the
/// leftmost, their middles with the middle of them all, and so on. A group
/// moves as one, but for the group `within`. None for fewer than two things
/// to line up.
pub fn align(
    diagram: &Diagram,
    ids: &[ElementId],
    to: Align,
    within: Option<u64>,
    extent: Extent<'_>,
) -> Vec<Change> {
    let units = units(diagram, ids, within, extent);
    let Some(all) = units
        .iter()
        .map(|(bounds, _)| *bounds)
        .reduce(|all, bounds| all.union(&bounds))
        .filter(|_| units.len() >= 2)
    else {
        return Vec::new();
    };
    let mut changes = Vec::new();
    for (bounds, members) in units {
        let (dx, dy) = match to {
            Align::Left => (all.x - bounds.x, 0.0),
            Align::Center => (all.center().x - bounds.center().x, 0.0),
            Align::Right => (all.right() - bounds.right(), 0.0),
            Align::Top => (0.0, all.y - bounds.y),
            Align::Middle => (0.0, all.center().y - bounds.center().y),
            Align::Bottom => (0.0, all.bottom() - bounds.bottom()),
        };
        if dx != 0.0 || dy != 0.0 {
            changes.extend(members.into_iter().map(|element| Change::Replace {
                element: translate(element, dx, dy),
            }));
        }
    }
    changes
}

// ── Snapping ─────────────────────────────────────────────────────────────────

/// How far to move something so it lines up with something else, and the
/// lines it then shares with it (to draw as guides).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snap {
    pub dx: f32,
    pub dy: f32,
    /// The x of the upright line it lines up on, if any.
    pub x: Option<f32>,
    /// The y of the level line.
    pub y: Option<f32>,
}

/// The boxes things snap to: the diagram's boxes and text, but for `not`.
pub fn snap_targets(diagram: &Diagram, not: &[ElementId], extent: Extent<'_>) -> Vec<Rect> {
    diagram
        .elements()
        .iter()
        .filter(|element| !not.contains(&element.id))
        .filter(|element| element.is_box() || matches!(element.shape, Shape::Text { .. }))
        .map(extent)
        .collect()
}

/// The nearest of `to` to any of `from`, within `reach`: how far, and where.
fn nearest(from: &[f32], to: impl Iterator<Item = f32> + Clone, reach: f32) -> Option<(f32, f32)> {
    from.iter()
        .flat_map(|from| to.clone().map(move |to| (to - from, to)))
        .filter(|(by, _)| by.abs() <= reach)
        .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
}

/// Snap a box being moved: its sides and middle to the others' sides and
/// middles, each way, if any are within `reach`.
pub fn snap_rect(moving: Rect, others: &[Rect], reach: f32) -> Snap {
    let across = |rect: &Rect| [rect.x, rect.center().x, rect.right()];
    let down = |rect: &Rect| [rect.y, rect.center().y, rect.bottom()];
    let x = nearest(&across(&moving), others.iter().flat_map(across), reach);
    let y = nearest(&down(&moving), others.iter().flat_map(down), reach);
    Snap {
        dx: x.map_or(0.0, |(by, _)| by),
        dy: y.map_or(0.0, |(by, _)| by),
        x: x.map(|(_, at)| at),
        y: y.map(|(_, at)| at),
    }
}

/// Snap a point being dragged (a corner of a box being made or resized) to
/// the others' sides and middles.
pub fn snap_point(at: Point, others: &[Rect], reach: f32) -> Snap {
    snap_rect(Rect::new(at.x, at.y, 0.0, 0.0), others, reach)
}
