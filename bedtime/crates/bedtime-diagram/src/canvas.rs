//! The canvas: a diagram drawn through a camera that pans and zooms, and the
//! pointer and keys that edit it. It draws the caller's diagram and never
//! changes it: a finished gesture is returned as an `Edit` for the caller to
//! apply, so one drag is one step of undo. Until then the gesture is drawn as
//! a preview over the diagram as it is.
use crate::edit::{self, Align, Extent, Handle, Order, Restyle, Snap, Style, Tool};
use crate::model::{
    Change, Color, Dash, Diagram, Element, ElementId, Head, Point, Rect, Shape, Side, TEXT_SIZE,
};
use bedtime_ui::{
    Floating, UiContext, UiFrame,
    egui::{self, Color32, Pos2, Stroke, Vec2, epaint},
    theme::{Theme, color},
};

/// How far the camera zooms out and in.
pub const ZOOM: std::ops::RangeInclusive<f32> = 0.1..=8.0;
/// What Zoom In and Zoom Out multiply the zoom by.
const ZOOM_STEP: f32 = 1.25;
/// The room left around a diagram fitted to the canvas, in points.
const FIT_MARGIN: f32 = 48.0;
/// The grid's dots are this far apart on the canvas (doubled as often as it
/// takes to keep them apart on screen).
const GRID: f32 = 20.0;

/// Where the canvas is looked at from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// The canvas point in the middle of the view.
    pub center: Point,
    /// Screen points per canvas unit.
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: Point::default(),
            zoom: 1.0,
        }
    }
}

impl Camera {
    /// Showing all of `bounds` in a view of `size`, no closer than actual
    /// size; the origin for an empty diagram.
    pub fn fitting(bounds: Option<Rect>, size: Vec2) -> Self {
        let Some(bounds) = bounds else {
            return Self::default();
        };
        let room = (size - Vec2::splat(2.0 * FIT_MARGIN)).max(Vec2::splat(1.0));
        let zoom = (room.x / bounds.w.max(1.0)).min(room.y / bounds.h.max(1.0));
        Self {
            center: bounds.center(),
            zoom: zoom.clamp(*ZOOM.start(), 1.0),
        }
    }

    /// Where a canvas point is in a view filling `view`.
    pub fn to_screen(&self, view: egui::Rect, point: Point) -> Pos2 {
        view.center()
            + Vec2::new(point.x - self.center.x, point.y - self.center.y) * self.zoom
    }

    /// The canvas point at a place on screen.
    pub fn to_canvas(&self, view: egui::Rect, at: Pos2) -> Point {
        let offset = (at - view.center()) / self.zoom;
        Point::new(self.center.x + offset.x, self.center.y + offset.y)
    }

    /// Move the view by a distance on screen (dragging the canvas).
    pub fn pan(&mut self, by: Vec2) {
        self.center.x -= by.x / self.zoom;
        self.center.y -= by.y / self.zoom;
    }

    /// Zoom by `factor`, keeping the canvas point under `at` where it is.
    pub fn zoom_at(&mut self, view: egui::Rect, at: Pos2, factor: f32) {
        let held = self.to_canvas(view, at);
        self.zoom = (self.zoom * factor).clamp(*ZOOM.start(), *ZOOM.end());
        let moved = self.to_screen(view, held);
        self.pan(at - moved);
    }

    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom * ZOOM_STEP).min(*ZOOM.end());
    }

    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom / ZOOM_STEP).max(*ZOOM.start());
    }
}

/// How near something dragged must come to lining up with something else to
/// snap to it, in screen points.
const SNAP_REACH: f32 = 6.0;
/// How near the pointer must be to pick something, in screen points.
const REACH: f32 = 6.0;
/// The gap between a selection and the frame around it, in screen points.
const FRAME_GAP: f32 = 5.0;
/// A grip's size on screen.
const GRIP: f32 = 8.0;
/// A side shorter than this on screen has no grip in its middle.
const SIDE_GRIPS_FROM: f32 = 40.0;

/// What the canvas keeps between frames besides the camera: the tool, the
/// selection, and the gesture under way.
#[derive(Default)]
pub struct State {
    pub tool: Tool,
    /// The selected elements, in the order they were picked.
    pub selection: Vec<ElementId>,
    /// The style of what's made next: the last picked.
    pub style: Style,
    /// How near (on the canvas) something dragged snaps to the sides and
    /// middles of other things; `None` while snapping is held off.
    pub(crate) snap: Option<f32>,
    /// A move leaves what's moved where it was, and moves copies.
    pub(crate) copy: bool,
    gesture: Option<Gesture>,
    /// The text being written, if any: the canvas waits for it.
    pub(crate) writing: Option<Writing>,
}

/// Text being written in place, and what it's for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Writing {
    pub(crate) subject: Subject,
    pub(crate) text: String,
    /// Whether its field has been given the keyboard yet.
    focused: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Subject {
    /// New text, with its top left corner here.
    New(Point),
    /// An element's text: a text element's own, or a box's label.
    Of(ElementId),
}

/// Which kinds of style there are to pick, besides the color of the stroke
/// (everything has that).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Styles {
    /// A box's fill.
    fill: bool,
    /// A stroke's width and dashes: anything but text.
    stroke: bool,
    /// A rectangle's corners.
    corners: bool,
    /// Text's size.
    size: bool,
    /// Something is selected, to move forward or back.
    order: bool,
    /// Two things or more are selected, to line up.
    align: bool,
}

/// What was picked in the style bar.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pick {
    Style(Restyle),
    Order(Order),
    Align(Align),
}

/// A finished gesture's changes to the diagram: one step of undo.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    /// What undo calls it: "Move", "Add Rectangle".
    pub label: String,
    pub changes: Vec<Change>,
}

impl Edit {
    /// With what follows for lines bound to what it changes.
    pub(crate) fn bound(mut self, diagram: &Diagram, extent: Extent<'_>) -> Self {
        self.changes = edit::with_bindings(diagram, self.changes, extent);
        self
    }
}

/// A drag under way. Points are on the canvas.
#[derive(Clone, Debug)]
enum Gesture {
    Pan,
    /// Selecting what a box encloses, besides `keep`.
    Marquee {
        from: Point,
        to: Point,
        keep: Vec<ElementId>,
    },
    /// Moving the selection.
    Move { from: Point, to: Point },
    /// Stretching the selection's box `from` to `to` by a handle, held
    /// `grab` away from the handle's place.
    Resize {
        handle: Handle,
        from: Rect,
        to: Rect,
        grab: (f32, f32),
    },
    /// Moving one point of a line.
    MovePoint {
        id: ElementId,
        index: usize,
        to: Point,
    },
    /// Drawing a stroke by hand: where the pointer has been, and how far
    /// from them the stroke kept may stray.
    Draw { points: Vec<Point>, tolerance: f32 },
    /// Making an element with the tool.
    Create {
        from: Point,
        to: Point,
        constrain: bool,
    },
}

/// Something of the selection to drag: a handle of its box, or a line's point.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grip {
    Box(Handle),
    Point(usize),
}

impl State {
    pub fn is_selected(&self, id: ElementId) -> bool {
        self.selection.contains(&id)
    }

    /// Take a tool in hand. A tool that makes things lets go of the
    /// selection: the styles shown are then the ones it will make them in.
    pub fn pick(&mut self, tool: Tool) {
        self.tool = tool;
        if tool != Tool::Select {
            self.selection.clear();
        }
    }

    /// Whether a drag is under way.
    pub fn is_busy(&self) -> bool {
        self.gesture.is_some()
    }

    /// A style picked: it's the style of what's made next, and of what's
    /// selected, as far as each takes it.
    pub(crate) fn restyle(&mut self, diagram: &Diagram, change: Restyle) -> Option<Edit> {
        self.style.set(change);
        let changes: Vec<Change> = self
            .selected(diagram)
            .into_iter()
            .map(|element| edit::restyle(element, change))
            .filter(|styled| diagram.get(styled.id) != Some(styled))
            .map(|element| Change::Replace { element })
            .collect();
        (!changes.is_empty()).then(|| Edit {
            label: "Style".into(),
            changes,
        })
    }

    /// Which styles there are to pick, and the ones in use: those of what's
    /// selected, else of what the tool in hand makes. `None` when there's
    /// nothing to style.
    fn styles(&self, diagram: &Diagram) -> Option<(Styles, Style)> {
        let selected = self.selected(diagram);
        if selected.is_empty() {
            let made = |fill, stroke, corners, size| Styles {
                fill,
                stroke,
                corners,
                size,
                order: false,
                align: false,
            };
            let styles = match self.tool {
                Tool::Select => return None,
                Tool::Rectangle => made(true, true, true, false),
                Tool::Ellipse | Tool::Diamond => made(true, true, false, false),
                Tool::Line | Tool::Arrow | Tool::Pen => made(false, true, false, false),
                Tool::Text => made(false, false, false, true),
            };
            return Some((styles, self.style));
        }
        let is_text = |element: &&Element| matches!(element.shape, Shape::Text { .. });
        let styles = Styles {
            fill: selected.iter().any(|element| element.is_box()),
            stroke: selected.iter().any(|element| !is_text(element)),
            corners: selected
                .iter()
                .any(|element| matches!(element.shape, Shape::Rectangle(_))),
            size: selected.iter().any(is_text),
            order: true,
            align: edit::unit_count(diagram, &self.selection) >= 2,
        };
        // Each kind of style from the first selected element that has it.
        let style = selected
            .iter()
            .rev()
            .fold(self.style, |style, element| style.of(element));
        Some((styles, style))
    }

    /// Whether text is being written.
    pub fn is_writing(&self) -> bool {
        self.writing.is_some()
    }

    /// Begin writing: new text, or an element's (a line has none to write).
    pub(crate) fn write(&mut self, diagram: &Diagram, subject: Subject) {
        let text = match subject {
            Subject::New(_) => String::new(),
            Subject::Of(id) => match diagram.get(id) {
                Some(Element {
                    shape: Shape::Text { text, .. },
                    ..
                }) => text.clone(),
                Some(element) if element.takes_label() => element.label.clone(),
                _ => return,
            },
        };
        self.gesture = None;
        self.writing = Some(Writing {
            subject,
            text,
            focused: false,
        });
    }

    /// Done writing: the edit that keeps what was written, if it changed
    /// anything. New text with nothing in it is nothing; a text element left
    /// empty is removed.
    pub(crate) fn written(&mut self, diagram: &Diagram) -> Option<Edit> {
        let Writing { subject, text, .. } = self.writing.take()?;
        let text = text.trim_end().to_owned();
        match subject {
            Subject::New(at) => {
                self.tool = Tool::Select;
                if text.trim().is_empty() {
                    return None;
                }
                let element = self.style.dress(&Element::new(
                    diagram.next_id(),
                    Shape::Text {
                        at,
                        text,
                        size: TEXT_SIZE,
                    },
                ));
                self.selection = vec![element.id];
                Some(Edit {
                    label: "Add Text".into(),
                    changes: vec![Change::Insert {
                        index: diagram.elements().len(),
                        element,
                    }],
                })
            }
            Subject::Of(id) => {
                let before = diagram.get(id)?;
                let mut element = before.clone();
                match &mut element.shape {
                    Shape::Text { text: old, .. } => {
                        if text.trim().is_empty() {
                            self.selection.retain(|other| *other != id);
                            return Some(Edit {
                                label: "Delete".into(),
                                changes: vec![Change::Remove { id }],
                            });
                        }
                        *old = text;
                    }
                    _ => element.label = text,
                }
                (element != *before).then(|| Edit {
                    label: "Edit Text".into(),
                    changes: vec![Change::Replace { element }],
                })
            }
        }
    }

    /// What the gesture under way changes, as it would leave it: the moved,
    /// the resized, a line with its point elsewhere. A line's end dragged
    /// onto an element is bound to it, and off it, unbound; a line moved
    /// without what it's bound to lets go.
    fn changes(&self, diagram: &Diagram, extent: Extent<'_>) -> Vec<Change> {
        let Some(gesture) = &self.gesture else {
            return Vec::new();
        };
        let snap = self.snapping(diagram, extent);
        if let Gesture::Move { from, to } = gesture
            && self.copy
        {
            // The selection stays; copies of it are what moved.
            let (dx, dy) = (to.x - from.x + snap.dx, to.y - from.y + snap.dy);
            if dx == 0.0 && dy == 0.0 {
                return Vec::new();
            }
            let copies = edit::copies(&self.selected(diagram), diagram, dx, dy);
            return edit::insertions(diagram, copies);
        }
        diagram
            .elements()
            .iter()
            .filter_map(|element| {
                let mut changed = match gesture {
                    Gesture::Move { from, to } if self.is_selected(element.id) => {
                        edit::translate(element, to.x - from.x + snap.dx, to.y - from.y + snap.dy)
                    }
                    Gesture::Resize {
                        handle, from, to, ..
                    } if self.is_selected(element.id) => {
                        let corner = dragged_corner(*handle, *to);
                        let to = handle.drag(*to, Point::new(corner.x + snap.dx, corner.y + snap.dy));
                        edit::remap(element, *from, to)
                    }
                    Gesture::MovePoint { id, index, to } if *id == element.id => {
                        let mut moved = edit::with_point(element, *index, *to);
                        let onto = edit::binding_at(diagram, *to, extent, Some(*id));
                        edit::bind_end(&mut moved, *index, onto);
                        moved
                    }
                    _ => return None,
                };
                if matches!(gesture, Gesture::Move { .. } | Gesture::Resize { .. }) {
                    edit::unbind_unless(&mut changed, |id| self.is_selected(id));
                }
                (changed != *element).then_some(Change::Replace { element: changed })
            })
            .collect()
    }

    /// How what the gesture under way drags snaps to the sides and middles
    /// of the things it isn't dragging: a moved selection's box, the corner
    /// of a box being made or resized. Nothing, with snapping held off.
    fn snapping(&self, diagram: &Diagram, extent: Extent<'_>) -> Snap {
        let Some(reach) = self.snap else {
            return Snap::default();
        };
        match &self.gesture {
            Some(Gesture::Move { from, to }) => {
                let Some(held) = bounds(&self.selected(diagram), extent) else {
                    return Snap::default();
                };
                let moved = Rect::new(held.x + to.x - from.x, held.y + to.y - from.y, held.w, held.h);
                // Copies snap to what they're copies of, too.
                let not: &[ElementId] = if self.copy { &[] } else { &self.selection };
                edit::snap_rect(moved, &edit::snap_targets(diagram, not, extent), reach)
            }
            Some(Gesture::Resize { handle, to, .. }) => {
                let others = edit::snap_targets(diagram, &self.selection, extent);
                let mut snap = edit::snap_point(dragged_corner(*handle, *to), &others, reach);
                // A side's handle moves one way only.
                if handle.x == 0 {
                    (snap.dx, snap.x) = (0.0, None);
                }
                if handle.y == 0 {
                    (snap.dy, snap.y) = (0.0, None);
                }
                snap
            }
            Some(Gesture::Create { to, .. })
                if matches!(self.tool, Tool::Rectangle | Tool::Ellipse | Tool::Diamond) =>
            {
                edit::snap_point(*to, &edit::snap_targets(diagram, &[], extent), reach)
            }
            _ => Snap::default(),
        }
    }

    /// The selection without what's gone, and with everything grouped with
    /// what's in it.
    fn settle_selection(&mut self, diagram: &Diagram) {
        self.selection.retain(|id| diagram.get(*id).is_some());
        self.selection = edit::with_groups(diagram, &self.selection);
    }

    /// The edit that removes the selection.
    fn remove_selected(&mut self, diagram: &Diagram, label: &str) -> Option<Edit> {
        let changes: Vec<Change> = self
            .selected(diagram)
            .iter()
            .map(|element| Change::Remove { id: element.id })
            .collect();
        self.selection.clear();
        (!changes.is_empty()).then(|| Edit {
            label: label.into(),
            changes,
        })
    }

    /// The edit that adds elements in front, which are then the selection.
    fn add(&mut self, diagram: &Diagram, elements: Vec<Element>, label: &str) -> Option<Edit> {
        if elements.is_empty() {
            return None;
        }
        self.tool = Tool::Select;
        self.selection = elements.iter().map(|element| element.id).collect();
        Some(Edit {
            label: label.into(),
            changes: edit::insertions(diagram, elements),
        })
    }

    /// Copies of the selection, a little down and to the right of it.
    pub(crate) fn duplicate(&mut self, diagram: &Diagram) -> Option<Edit> {
        let copies = edit::copies(&self.selected(diagram), diagram, 10.0, 10.0);
        self.add(diagram, copies, "Duplicate")
    }

    /// The selection as text for the clipboard.
    pub(crate) fn copied(&self, diagram: &Diagram) -> Option<String> {
        let selected = self.selected(diagram);
        (!selected.is_empty()).then(|| edit::to_text(&selected))
    }

    /// What's on the clipboard, with its middle at `at`: a diagram's
    /// elements, or any other (short) text as text.
    pub(crate) fn paste(
        &mut self,
        diagram: &Diagram,
        text: &str,
        at: Point,
        extent: Extent<'_>,
    ) -> Option<Edit> {
        let Some(elements) = edit::from_text(text) else {
            let text = text.trim_end();
            if text.trim().is_empty() || text.len() > 4000 {
                return None;
            }
            let written = self.style.dress(&Element::new(
                diagram.next_id(),
                Shape::Text {
                    at,
                    text: text.to_owned(),
                    size: TEXT_SIZE,
                },
            ));
            return self.add(diagram, vec![written], "Paste");
        };
        let pasted: Vec<&Element> = elements.iter().collect();
        let middle = bounds(&pasted, extent)?.center();
        let copies = edit::copies(&pasted, diagram, at.x - middle.x, at.y - middle.y);
        self.add(diagram, copies, "Paste")
    }

    /// The selection moved forward or back among the rest.
    pub(crate) fn reorder(&self, diagram: &Diagram, order: Order) -> Option<Edit> {
        let changes = edit::reorder(diagram, &self.selection, order);
        (!changes.is_empty()).then(|| Edit {
            label: match order {
                Order::Front => "Bring to Front",
                Order::Forward => "Bring Forward",
                Order::Backward => "Send Backward",
                Order::Back => "Send to Back",
            }
            .into(),
            changes,
        })
    }

    /// The selection made one group, or taken out of its groups.
    pub(crate) fn group(&self, diagram: &Diagram, together: bool) -> Option<Edit> {
        let (changes, label) = if together {
            (edit::group(diagram, &self.selection), "Group")
        } else {
            (edit::ungroup(diagram, &self.selection), "Ungroup")
        };
        (!changes.is_empty()).then(|| Edit {
            label: label.into(),
            changes,
        })
    }

    /// The selection lined up.
    pub(crate) fn align(&self, diagram: &Diagram, to: Align, extent: Extent<'_>) -> Option<Edit> {
        let changes = edit::align(diagram, &self.selection, to, extent);
        (!changes.is_empty()).then(|| Edit {
            label: "Align".into(),
            changes,
        })
    }

    /// The element the tool would make by a drag: a line's ends bound to
    /// what they're on.
    fn making(
        &self,
        diagram: &Diagram,
        from: Point,
        to: Point,
        constrain: bool,
        extent: Extent<'_>,
    ) -> Option<Element> {
        let snap = self.snapping(diagram, extent);
        let to = Point::new(to.x + snap.dx, to.y + snap.dy);
        let mut made = edit::create(self.tool, diagram.next_id(), from, to, constrain)?;
        if matches!(made.shape, Shape::Line { .. }) {
            edit::bind_end(&mut made, 0, edit::binding_at(diagram, from, extent, None));
            edit::bind_end(&mut made, 1, edit::binding_at(diagram, to, extent, None));
            made = edit::settle(&made, diagram, extent);
        }
        Some(self.style.dress(&made))
    }

    /// What a line's end being dragged would bind to, and the side it would
    /// be pinned to, to show it.
    fn binding_to(&self, diagram: &Diagram, extent: Extent<'_>) -> Option<(ElementId, Side)> {
        match self.gesture.as_ref()? {
            Gesture::Create { to, .. } if matches!(self.tool, Tool::Line | Tool::Arrow) => {
                edit::binding_at(diagram, *to, extent, None)
            }
            Gesture::MovePoint { id, index, to } => {
                let Shape::Line { points, .. } = &diagram.get(*id)?.shape else {
                    return None;
                };
                (*index == 0 || index + 1 == points.len())
                    .then(|| edit::binding_at(diagram, *to, extent, Some(*id)))
                    .flatten()
            }
            _ => None,
        }
    }

    /// The selected elements, back to front.
    fn selected<'a>(&self, diagram: &'a Diagram) -> Vec<&'a Element> {
        diagram
            .elements()
            .iter()
            .filter(|element| self.is_selected(element.id))
            .collect()
    }

    /// A press at `origin` (on screen) that became a drag: the tool's gesture
    /// begins.
    fn press(
        &mut self,
        diagram: &Diagram,
        view: egui::Rect,
        camera: &Camera,
        origin: Pos2,
        shift: bool,
        extent: Extent<'_>,
    ) {
        if self.tool == Tool::Text {
            self.write_at(diagram, camera.to_canvas(view, origin));
            return;
        }
        self.gesture = Some(if self.tool == Tool::Select {
            self.begin(diagram, view, camera, origin, shift, extent)
        } else if self.tool == Tool::Pen {
            self.selection.clear();
            Gesture::Draw {
                points: vec![camera.to_canvas(view, origin)],
                // Under a point on screen, however closely it's looked at.
                tolerance: 0.75 / camera.zoom,
            }
        } else {
            let from = camera.to_canvas(view, origin);
            self.selection.clear();
            Gesture::Create {
                from,
                to: from,
                constrain: shift,
            }
        });
    }

    /// Begin new text whose first line has `at` at its left, halfway up.
    fn write_at(&mut self, diagram: &Diagram, at: Point) {
        self.selection.clear();
        self.write(
            diagram,
            Subject::New(Point::new(at.x, at.y - TEXT_SIZE * 0.65)),
        );
    }

    /// A whole drag from one place on screen to another, as the pointer
    /// would make it.
    #[cfg(test)]
    pub(crate) fn drag(
        &mut self,
        diagram: &Diagram,
        view: egui::Rect,
        camera: &mut Camera,
        from: Pos2,
        to: Pos2,
        shift: bool,
    ) -> Option<Edit> {
        self.press(diagram, view, camera, from, shift, &Element::bounds);
        self.follow(view, camera, to, to - from, shift);
        self.finish(diagram, &Element::bounds)
            .map(|edit| edit.bound(diagram, &Element::bounds))
    }

    /// A press with the Select tool at `origin` (on screen) that became a
    /// drag: what it drags.
    fn begin(
        &mut self,
        diagram: &Diagram,
        view: egui::Rect,
        camera: &Camera,
        origin: Pos2,
        extend: bool,
        extent: Extent<'_>,
    ) -> Gesture {
        let at = camera.to_canvas(view, origin);
        let selected = self.selected(diagram);
        let (_, grips) = grips(&selected, view, camera, extent);
        if let Some((grip, _)) = grips
            .iter()
            .find(|(_, place)| (*place - origin).length() <= REACH + GRIP / 2.0)
        {
            match *grip {
                Grip::Point(index) => {
                    return Gesture::MovePoint {
                        id: selected[0].id,
                        index,
                        to: at,
                    };
                }
                Grip::Box(handle) => {
                    if let Some(from) = bounds(&selected, extent) {
                        let place = handle_place(handle, from);
                        return Gesture::Resize {
                            handle,
                            from,
                            to: from,
                            grab: (at.x - place.x, at.y - place.y),
                        };
                    }
                }
            }
        }
        let reach = REACH / camera.zoom;
        if let Some(id) = edit::topmost_at(diagram, at, reach, extent) {
            if !self.is_selected(id) {
                if !extend {
                    self.selection.clear();
                }
                self.selection.push(id);
                self.settle_selection(diagram);
            }
            return Gesture::Move { from: at, to: at };
        }
        if bounds(&selected, extent).is_some_and(|bounds| bounds.contains(at)) {
            return Gesture::Move { from: at, to: at };
        }
        if !extend {
            self.selection.clear();
        }
        Gesture::Marquee {
            from: at,
            to: at,
            keep: self.selection.clone(),
        }
    }

    /// The gesture followed to where the pointer is.
    fn follow(&mut self, view: egui::Rect, camera: &mut Camera, pointer: Pos2, by: Vec2, constrain: bool) {
        let at = camera.to_canvas(view, pointer);
        match self.gesture.as_mut() {
            Some(Gesture::Pan) => camera.pan(by),
            Some(Gesture::Marquee { to, .. } | Gesture::Move { to, .. })
            | Some(Gesture::MovePoint { to, .. }) => *to = at,
            Some(Gesture::Create {
                to, constrain: held, ..
            }) => {
                *to = at;
                *held = constrain;
            }
            Some(Gesture::Resize {
                handle,
                from,
                to,
                grab,
            }) => *to = handle.drag(*from, Point::new(at.x - grab.0, at.y - grab.1)),
            Some(Gesture::Draw { points, .. }) => {
                // A point for each point of the screen the pointer crosses.
                let far = points.last().is_none_or(|last| {
                    (at.x - last.x).hypot(at.y - last.y) * camera.zoom >= 1.0
                });
                if far {
                    points.push(at);
                }
            }
            None => {}
        }
    }

    /// The gesture let go of: the selection it leaves, and the edit it makes.
    fn finish(&mut self, diagram: &Diagram, extent: Extent<'_>) -> Option<Edit> {
        let label = match self.gesture.as_ref()? {
            Gesture::Pan => {
                self.gesture = None;
                return None;
            }
            Gesture::Marquee { from, to, keep } => {
                let area = Rect::new(from.x, from.y, to.x - from.x, to.y - from.y);
                let mut selection = keep.clone();
                for id in edit::enclosed(diagram, area, extent) {
                    if !selection.contains(&id) {
                        selection.push(id);
                    }
                }
                self.selection = selection;
                self.settle_selection(diagram);
                self.gesture = None;
                return None;
            }
            Gesture::Create {
                from,
                to,
                constrain,
            } => {
                let made = self.making(diagram, *from, *to, *constrain, extent);
                let label = format!("Add {}", self.tool.label());
                self.gesture = None;
                let element = made?;
                // What's made is selected, ready to move, and the tool is put down.
                self.selection = vec![element.id];
                self.tool = Tool::Select;
                return Some(Edit {
                    label,
                    changes: vec![Change::Insert {
                        index: diagram.elements().len(),
                        element,
                    }],
                });
            }
            Gesture::Draw { points, tolerance } => {
                let made = edit::stroke(diagram.next_id(), points, *tolerance);
                self.gesture = None;
                // The pen stays in hand, for the next stroke.
                return made.map(|stroke| Edit {
                    label: "Draw".into(),
                    changes: vec![Change::Insert {
                        index: diagram.elements().len(),
                        element: self.style.dress(&stroke),
                    }],
                });
            }
            Gesture::Move { .. } => "Move",
            Gesture::Resize { .. } => "Resize",
            Gesture::MovePoint { .. } => "Move Point",
        };
        let changes = self.changes(diagram, extent);
        // Copies made by the move are what's selected after it.
        let copied: Vec<ElementId> = changes
            .iter()
            .filter_map(|change| match change {
                Change::Insert { element, .. } => Some(element.id),
                _ => None,
            })
            .collect();
        let label = if copied.is_empty() { label } else { "Duplicate" };
        if !copied.is_empty() {
            self.selection = copied;
        }
        self.gesture = None;
        (!changes.is_empty()).then(|| Edit {
            label: label.into(),
            changes,
        })
    }

    /// The keys, when the canvas has them: tools by letter, Escape, Delete,
    /// Enter, the arrows nudging the selection (further with Shift), and
    /// with Cmd/Ctrl: A (all), C, X, V (pasting at `paste_at`), D
    /// (duplicate), G (group; with Shift, ungroup), and ] and [ (forward and
    /// backward; with Shift, to the front and to the back).
    fn keys(
        &mut self,
        ui: &mut egui::Ui,
        diagram: &Diagram,
        paste_at: Point,
        extent: Extent<'_>,
    ) -> Option<Edit> {
        enum Ask {
            Copy,
            Cut,
            Paste(String),
            Duplicate,
            Group(bool),
            Order(Order),
        }
        let (mut delete, mut nudge, mut write, mut ask) = (false, Vec2::ZERO, None, None);
        ui.input_mut(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Copy => ask = Some(Ask::Copy),
                    egui::Event::Cut => ask = Some(Ask::Cut),
                    egui::Event::Paste(text) => ask = Some(Ask::Paste(text.clone())),
                    _ => {}
                }
            }
            let command = egui::Modifiers::COMMAND;
            let shifted = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
            if i.consume_key(command, egui::Key::A) {
                self.tool = Tool::Select;
                self.selection = diagram.elements().iter().map(|element| element.id).collect();
            }
            if i.consume_key(command, egui::Key::D) {
                ask = Some(Ask::Duplicate);
            }
            // With Shift first: without it, the plainer chord matches too.
            if i.consume_key(shifted, egui::Key::G) {
                ask = Some(Ask::Group(false));
            } else if i.consume_key(command, egui::Key::G) {
                ask = Some(Ask::Group(true));
            }
            if i.consume_key(shifted, egui::Key::CloseBracket) {
                ask = Some(Ask::Order(Order::Front));
            } else if i.consume_key(command, egui::Key::CloseBracket) {
                ask = Some(Ask::Order(Order::Forward));
            }
            if i.consume_key(shifted, egui::Key::OpenBracket) {
                ask = Some(Ask::Order(Order::Back));
            } else if i.consume_key(command, egui::Key::OpenBracket) {
                ask = Some(Ask::Order(Order::Backward));
            }
            if i.modifiers.command || i.modifiers.ctrl || i.modifiers.alt {
                return;
            }
            for tool in Tool::ALL {
                if i.key_pressed(tool_key(tool)) {
                    self.pick(tool);
                }
            }
            if i.key_pressed(egui::Key::Escape) {
                // First the drag, then the tool, then the selection.
                if self.gesture.take().is_none() {
                    if self.tool != Tool::Select {
                        self.tool = Tool::Select;
                    } else {
                        self.selection.clear();
                    }
                }
            }
            delete = i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace);
            // Enter writes in what's selected.
            if let [only] = self.selection[..]
                && self.gesture.is_none()
                && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
            {
                write = Some(only);
            }
            let step = if i.modifiers.shift { 10.0 } else { 1.0 };
            for (key, way) in [
                (egui::Key::ArrowLeft, Vec2::LEFT),
                (egui::Key::ArrowRight, Vec2::RIGHT),
                (egui::Key::ArrowUp, Vec2::UP),
                (egui::Key::ArrowDown, Vec2::DOWN),
            ] {
                if i.key_pressed(key) {
                    nudge += way * step;
                }
            }
        });
        if let Some(id) = write {
            self.write(diagram, Subject::Of(id));
            return None;
        }
        if self.gesture.is_some() {
            return None;
        }
        self.settle_selection(diagram);
        match ask {
            Some(Ask::Paste(text)) => return self.paste(diagram, &text, paste_at, extent),
            Some(Ask::Copy | Ask::Cut) => {
                if let Some(text) = self.copied(diagram) {
                    ui.ctx().copy_text(text);
                }
                if matches!(ask, Some(Ask::Cut)) {
                    return self.remove_selected(diagram, "Cut");
                }
                return None;
            }
            Some(Ask::Duplicate) => return self.duplicate(diagram),
            Some(Ask::Group(together)) => return self.group(diagram, together),
            Some(Ask::Order(order)) => return self.reorder(diagram, order),
            None => {}
        }
        if delete {
            return self.remove_selected(diagram, "Delete");
        }
        let selected = self.selected(diagram);
        (nudge != Vec2::ZERO && !selected.is_empty()).then(|| Edit {
            label: "Move".into(),
            changes: selected
                .iter()
                .map(|element| Change::Replace {
                    element: edit::translate(element, nudge.x, nudge.y),
                })
                .collect(),
        })
    }
}

/// The corner (or side) of a box that a handle drags.
fn dragged_corner(handle: Handle, rect: Rect) -> Point {
    Point::new(
        if handle.x < 0 { rect.x } else { rect.right() },
        if handle.y < 0 { rect.y } else { rect.bottom() },
    )
}

fn tool_key(tool: Tool) -> egui::Key {
    match tool {
        Tool::Select => egui::Key::V,
        Tool::Rectangle => egui::Key::R,
        Tool::Ellipse => egui::Key::O,
        Tool::Diamond => egui::Key::D,
        Tool::Line => egui::Key::L,
        Tool::Arrow => egui::Key::A,
        Tool::Text => egui::Key::T,
        Tool::Pen => egui::Key::P,
    }
}

/// The box around some elements.
fn bounds(elements: &[&Element], extent: Extent<'_>) -> Option<Rect> {
    elements
        .iter()
        .map(|element| extent(element))
        .reduce(|all, bounds| all.union(&bounds))
}

/// Where a handle is on a box.
fn handle_place(handle: Handle, rect: Rect) -> Point {
    let along = |way: i8, from: f32, size: f32| from + size * (f32::from(way) + 1.0) / 2.0;
    Point::new(along(handle.x, rect.x, rect.w), along(handle.y, rect.y, rect.h))
}

/// The frame around a selection and its grips, on screen. One line alone has
/// a grip on each of its points, and no frame; text alone has a frame and no
/// grips (it can't be stretched).
fn grips(
    selected: &[&Element],
    view: egui::Rect,
    camera: &Camera,
    extent: Extent<'_>,
) -> (Option<egui::Rect>, Vec<(Grip, Pos2)>) {
    if let [only] = selected
        && let Shape::Line { points, .. } = &only.shape
    {
        let grips = points
            .iter()
            .enumerate()
            .map(|(index, point)| (Grip::Point(index), camera.to_screen(view, *point)))
            .collect();
        return (None, grips);
    }
    let Some(bounds) = bounds(selected, extent) else {
        return (None, Vec::new());
    };
    let frame = on_screen(view, camera, bounds).expand(FRAME_GAP);
    if selected
        .iter()
        .all(|element| matches!(element.shape, Shape::Text { .. }))
    {
        return (Some(frame), Vec::new());
    }
    let framed = Rect::new(frame.min.x, frame.min.y, frame.width(), frame.height());
    let grips = Handle::ALL
        .into_iter()
        .filter(|handle| {
            handle.is_corner()
                || (handle.x == 0 && frame.width() >= SIDE_GRIPS_FROM)
                || (handle.y == 0 && frame.height() >= SIDE_GRIPS_FROM)
        })
        .map(|handle| {
            let place = handle_place(handle, framed);
            (Grip::Box(handle), Pos2::new(place.x, place.y))
        })
        .collect();
    (Some(frame), grips)
}

/// Draw `diagram` filling what's left of `ui`, with its tools, and let the
/// pointer and (if `keys`) the keyboard work on it. Scrolling pans; pinching
/// (or scrolling with Cmd/Ctrl) zooms around the pointer; dragging with the
/// middle button, or with Space held, pans. Dragging otherwise is the tool's.
/// A camera with no zoom yet is fitted to the diagram first.
///
/// Returns the edit to make, when a gesture or a key asks for one.
pub fn show(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    diagram: &Diagram,
    camera: &mut Camera,
    state: &mut State,
    keys: bool,
) -> Option<Edit> {
    let theme = context.theme;
    let id = ui.id().with("bedtime.diagram");
    let (view, response) = ui.allocate_exact_size(
        ui.available_size().max(Vec2::splat(1.0)),
        egui::Sense::click_and_drag(),
    );
    if camera.zoom <= 0.0 {
        *camera = Camera::fitting(diagram.bounds(), view.size());
    }
    // What was selected may be gone (undone, or changed in the file), and
    // what's grouped with it comes with it.
    state.settle_selection(diagram);

    // Text is measured with the fonts, for picking it and framing it.
    let fonts = ui.ctx().clone();
    let extent = |element: &Element| measured(&fonts, element);
    let extent: Extent<'_> = &extent;

    // While text is being written the canvas waits: a press on it only ends
    // the writing.
    let writing = state.is_writing();
    let keys = keys
        && !writing
        && !ui.ctx().text_edit_focused()
        && !egui::Popup::is_any_open(ui.ctx());
    let (space, shift) = ui.input(|i| (keys && i.key_down(egui::Key::Space), i.modifiers.shift));
    // Alt moves copies; Cmd/Ctrl holds snapping off.
    let (alt, command) = ui.input(|i| (i.modifiers.alt, i.modifiers.command));
    state.copy = alt;
    state.snap = (!command).then(|| SNAP_REACH / camera.zoom);

    let hover = ui.input(|i| i.pointer.hover_pos());
    // What's pasted goes under the pointer, or else in the middle.
    let paste_at = camera.to_canvas(
        view,
        hover.filter(|at| view.contains(*at)).unwrap_or(view.center()),
    );
    let mut edit = if keys {
        state.keys(ui, diagram, paste_at, extent)
    } else {
        None
    };
    if response.hovered()
        && let Some(pointer) = hover
    {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if pinch != 1.0 {
            camera.zoom_at(view, pointer, pinch);
        }
        if scroll != Vec2::ZERO {
            camera.pan(scroll);
            ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        }
    }

    // A drag begins where the button went down, not where it was noticed.
    let origin = ui
        .input(|i| i.pointer.press_origin())
        .or(response.interact_pointer_pos());
    let mut pressed_away = false;
    if response.drag_started_by(egui::PointerButton::Middle)
        || (space && response.drag_started_by(egui::PointerButton::Primary))
    {
        state.gesture = Some(Gesture::Pan);
    } else if writing {
        pressed_away = response.clicked() || response.drag_started();
    } else if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = origin
    {
        state.press(diagram, view, camera, origin, shift, extent);
    }
    // A stroke goes through everywhere the pointer went since the last
    // frame, not just where it is now.
    if matches!(state.gesture, Some(Gesture::Draw { .. })) {
        let crossed: Vec<Pos2> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::PointerMoved(at) => Some(*at),
                    _ => None,
                })
                .collect()
        });
        for at in crossed {
            state.follow(view, camera, at, Vec2::ZERO, shift);
        }
    }
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        state.follow(view, camera, pointer, response.drag_delta(), shift);
    }
    if response.drag_stopped() {
        // Up to where the button came up, which may be past where the
        // pointer last was.
        if let Some(pointer) = response.interact_pointer_pos() {
            state.follow(view, camera, pointer, Vec2::ZERO, shift);
        }
        edit = state.finish(diagram, extent).or(edit);
    }
    let clicked_at = response
        .interact_pointer_pos()
        .filter(|_| !writing && response.clicked())
        .map(|pointer| camera.to_canvas(view, pointer));
    if let Some(at) = clicked_at {
        if state.tool == Tool::Text {
            state.write_at(diagram, at);
        } else if state.tool == Tool::Select
            && (response.double_clicked() || response.triple_clicked())
        {
            // Twice (or more, quickly) on text or in a box writes there; on
            // nothing, new text.
            match edit::writable_at(diagram, at, REACH / camera.zoom, extent) {
                Some(id) => {
                    state.selection = vec![id];
                    state.write(diagram, Subject::Of(id));
                }
                None if edit::topmost_at(diagram, at, REACH / camera.zoom, extent).is_none() => {
                    state.write_at(diagram, at);
                }
                None => {}
            }
        } else if state.tool == Tool::Select {
            match edit::topmost_at(diagram, at, REACH / camera.zoom, extent) {
                // Shift adds to the selection, or takes away from it.
                Some(id) if shift => {
                    match state.selection.iter().position(|other| *other == id) {
                        Some(index) => {
                            state.selection.remove(index);
                        }
                        None => state.selection.push(id),
                    }
                }
                Some(id) => state.selection = vec![id],
                None if shift => {}
                None => state.selection.clear(),
            }
        }
    }

    // What to draw: the diagram as the gesture would leave it, bound lines
    // following. On the frame a gesture ends (or a key edits), that's the
    // diagram as the edit leaves it: the caller makes the edit after this
    // draw, and drawing the diagram as it still is would show, for a frame
    // or as long as that frame stays up, what was just moved back where it
    // was, and what was just made not there.
    let pending = match &edit {
        Some(edit) => edit.changes.clone(),
        None => state.changes(diagram, extent),
    };
    let previewed = (!pending.is_empty()).then(|| {
        let mut previewed = diagram.clone();
        for change in edit::with_bindings(diagram, pending, extent) {
            let _ = bedtime_document::DocumentModel::apply(&mut previewed, &change);
        }
        previewed
    });
    let shown = previewed.as_ref().unwrap_or(diagram).elements();
    let selected: Vec<&Element> = shown
        .iter()
        .filter(|element| state.is_selected(element.id))
        .collect();
    let (frame, grips) = grips(&selected, view, camera, extent);

    let painter = ui.painter_at(view);
    grid(&painter, theme, view, camera);
    // What's in view, with room for the widest stroke and an arrow's head.
    let seen = view.expand(32.0 * camera.zoom.max(1.0));
    for element in shown {
        // Text is measured when drawn, so it isn't skipped by its rough box.
        let is_text = matches!(element.shape, Shape::Text { .. });
        if is_text || seen.intersects(on_screen(view, camera, element.bounds())) {
            // Text being written is in its field instead.
            let written = state
                .writing
                .as_ref()
                .is_some_and(|writing| writing.subject == Subject::Of(element.id));
            paint(&painter, theme, view, camera, element, !written);
        }
    }
    let accent = color(theme.palette.accent);
    match &state.gesture {
        Some(Gesture::Create {
            from,
            to,
            constrain,
        }) => {
            if let Some(making) = state.making(diagram, *from, *to, *constrain, extent) {
                paint(&painter, theme, view, camera, &making, true);
            }
        }
        Some(Gesture::Draw { points, .. }) => {
            let drawing = Element::new(
                ElementId(0),
                Shape::Freehand {
                    points: points.clone(),
                },
            );
            paint(&painter, theme, view, camera, &state.style.dress(&drawing), true);
        }
        Some(Gesture::Marquee { from, to, .. }) => {
            let area = egui::Rect::from_two_pos(
                camera.to_screen(view, *from),
                camera.to_screen(view, *to),
            );
            painter.rect(
                area,
                0,
                accent.gamma_multiply(0.12),
                Stroke::new(1.0, accent),
                egui::StrokeKind::Middle,
            );
        }
        _ => {}
    }

    // What's dragged has snapped into line with something: the lines.
    let snap = state.snapping(diagram, extent);
    let guide = Stroke::new(1.0, accent.gamma_multiply(0.7));
    if let Some(x) = snap.x {
        let x = camera.to_screen(view, Point::new(x, 0.0)).x;
        painter.vline(x, view.y_range(), guide);
    }
    if let Some(y) = snap.y {
        let y = camera.to_screen(view, Point::new(0.0, y)).y;
        painter.hline(view.x_range(), y, guide);
    }

    // What a line's end being dragged would bind to, and where on it: a dot
    // on the side it would be pinned to, or in the middle for none.
    if let Some((onto, side)) = state.binding_to(diagram, extent)
        && let Some(onto) = diagram.get(onto)
    {
        let rect = on_screen(view, camera, extent(onto));
        painter.rect_stroke(
            rect.expand(FRAME_GAP),
            3,
            Stroke::new(2.0, accent),
            egui::StrokeKind::Middle,
        );
        let dot = match side {
            Side::Auto => rect.center(),
            Side::Top => rect.center_top(),
            Side::Right => rect.right_center(),
            Side::Bottom => rect.center_bottom(),
            Side::Left => rect.left_center(),
        };
        painter.circle_filled(dot, 4.0, accent);
    }

    // The selection: each element outlined when there are several, a frame
    // around them all, and the grips.
    if selected.len() > 1 {
        for element in &selected {
            painter.rect_stroke(
                on_screen(view, camera, extent(element)).expand(2.0),
                0,
                Stroke::new(1.0, accent.gamma_multiply(0.6)),
                egui::StrokeKind::Middle,
            );
        }
    }
    if let Some(frame) = frame {
        painter.rect_stroke(frame, 0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
    }
    let backdrop = color(theme.palette.panel_background);
    for (grip, place) in &grips {
        let rect = egui::Rect::from_center_size(*place, Vec2::splat(GRIP));
        let radius = if matches!(grip, Grip::Point(_)) { 4 } else { 1 };
        painter.rect(
            rect,
            radius,
            backdrop,
            Stroke::new(1.5, accent),
            egui::StrokeKind::Middle,
        );
    }

    // The pointer says what a drag would do.
    let panning = matches!(state.gesture, Some(Gesture::Pan));
    if panning {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if response.hovered()
        && let Some(pointer) = hover
    {
        let at = camera.to_canvas(view, pointer);
        let over_grip = grips
            .iter()
            .find(|(_, place)| (*place - pointer).length() <= REACH + GRIP / 2.0)
            .map(|(grip, _)| *grip);
        let icon = if space {
            egui::CursorIcon::Grab
        } else if !matches!(state.tool, Tool::Select | Tool::Text) {
            egui::CursorIcon::Crosshair
        } else if let Some(grip) = over_grip {
            match grip {
                Grip::Point(_) => egui::CursorIcon::Crosshair,
                Grip::Box(Handle { x: 0, .. }) => egui::CursorIcon::ResizeVertical,
                Grip::Box(Handle { y: 0, .. }) => egui::CursorIcon::ResizeHorizontal,
                Grip::Box(Handle { x, y }) if x == y => egui::CursorIcon::ResizeNwSe,
                Grip::Box(_) => egui::CursorIcon::ResizeNeSw,
            }
        } else if state.tool == Tool::Text {
            egui::CursorIcon::Text
        } else if edit::topmost_at(diagram, at, REACH / camera.zoom, extent).is_some()
            || bounds(&selected, extent).is_some_and(|bounds| bounds.contains(at))
        {
            egui::CursorIcon::Move
        } else {
            egui::CursorIcon::Default
        };
        ui.ctx().set_cursor_icon(icon);
    }

    // For screen readers and scripts: what's shown, how closely, what's
    // selected and the tool in hand.
    response.widget_info(|| {
        let mut info =
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Diagram canvas");
        let count = diagram.elements().len();
        info.current_text_value = Some(format!(
            "{count} element{} at {:.0}%; {} selected; {}",
            if count == 1 { "" } else { "s" },
            camera.zoom * 100.0,
            state.selection.len(),
            if state.is_writing() {
                "Writing"
            } else {
                state.tool.label()
            },
        ));
        info
    });

    // The text being written, in a field where it will be.
    if state.is_writing() {
        let done = writer(ui, theme, view, camera, diagram, state) || pressed_away;
        if done {
            edit = state.written(diagram).or(edit);
        }
    }

    // The tools, over the top of the canvas.
    let mut frame = context.frame(ui);
    Floating::new(id.with("tools"), view, egui::Align2::CENTER_TOP)
        .shown(true)
        .show(&mut frame, |frame| {
            for tool in Tool::ALL {
                if tool_button(frame, tool, state.tool == tool).clicked() && !state.is_writing() {
                    state.pick(tool);
                }
            }
        });

    // The styles, at the canvas's bottom left (clear of the tools, however
    // narrow the panel): of the selection, or of what the tool in hand will
    // make.
    if !state.is_writing()
        && let Some((styles, style)) = state.styles(diagram)
    {
        let picked = Floating::new(id.with("styles"), view, egui::Align2::LEFT_BOTTOM)
            .shown(true)
            .show(&mut frame, |frame| style_bar(frame.ui, frame.theme, styles, style));
        // And arranging the selection, at the bottom right.
        let arranged = if styles.order {
            Floating::new(id.with("arrange"), view, egui::Align2::RIGHT_BOTTOM)
                .shown(true)
                .show(&mut frame, |frame| arrange_bar(frame.ui, frame.theme, styles))
        } else {
            None
        };
        let made = match picked.or(arranged) {
            Some(Pick::Style(change)) => state.restyle(diagram, change),
            Some(Pick::Order(order)) => state.reorder(diagram, order),
            Some(Pick::Align(to)) => state.align(diagram, to, extent),
            None => None,
        };
        edit = made.or(edit);
    }

    // An edit changes what's drawn: draw again once it's made, rather than
    // when something next happens.
    if edit.is_some() {
        ui.ctx().request_repaint();
    }
    // With what follows for the lines bound to what changes.
    edit.map(|edit| edit.bound(diagram, extent))
}

/// The widths a stroke can be picked to have, and what they're called.
const WIDTHS: [(f32, &str); 3] = [(1.0, "Thin"), (2.0, "Regular"), (4.0, "Bold")];
/// The sizes text can be picked to have.
const SIZES: [(f32, &str); 4] = [(16.0, "S"), (20.0, "M"), (28.0, "L"), (36.0, "XL")];

/// The style bar: a row for each kind of style there is to pick, with the
/// one in use marked. Returns what was picked.
fn style_bar(ui: &mut egui::Ui, theme: &Theme, styles: Styles, style: Style) -> Option<Pick> {
    let mut picked = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(3.0, 4.0);
        row(ui, theme, "Stroke", &mut |ui| {
            for named in Color::ALL {
                if swatch(ui, theme, "Stroke", Some(named), style.stroke == named) {
                    picked = Some(Pick::Style(Restyle::Stroke(named)));
                }
            }
        });
        if styles.fill {
            row(ui, theme, "Fill", &mut |ui| {
                for named in std::iter::once(None).chain(Color::ALL.map(Some)) {
                    if swatch(ui, theme, "Fill", named, style.fill == named) {
                        picked = Some(Pick::Style(Restyle::Fill(named)));
                    }
                }
            });
        }
        if styles.stroke {
            row(ui, theme, "Width", &mut |ui| {
                for (width, name) in WIDTHS {
                    let on = style.width == width;
                    if option(ui, theme, &format!("{name} stroke"), on, |painter, rect, ink| {
                        painter.hline(rect.x_range(), rect.center().y, Stroke::new(width, ink));
                    }) {
                        picked = Some(Pick::Style(Restyle::Width(width)));
                    }
                }
            });
            row(ui, theme, "Line", &mut |ui| {
                for (dash, name) in [
                    (Dash::Solid, "Solid"),
                    (Dash::Dashed, "Dashed"),
                    (Dash::Dotted, "Dotted"),
                ] {
                    if option(ui, theme, &format!("{name} stroke"), style.dash == dash, |painter, rect, ink| {
                        let ends = vec![rect.left_center(), rect.right_center()];
                        path(painter, ends, false, Stroke::new(1.5, ink), dashes(dash, 1.5));
                    }) {
                        picked = Some(Pick::Style(Restyle::Dash(dash)));
                    }
                }
            });
        }
        if styles.corners {
            row(ui, theme, "Corners", &mut |ui| {
                for (rounded, name) in [(false, "Sharp corners"), (true, "Round corners")] {
                    if option(ui, theme, name, style.rounded == rounded, |painter, rect, ink| {
                        // The top left corner of a box.
                        let corner = egui::Rect::from_min_size(
                            rect.left_top() + Vec2::new(3.0, 2.0),
                            Vec2::splat(rect.height() * 2.0),
                        );
                        let radius = if rounded { rect.height() * 0.6 } else { 0.0 };
                        painter.with_clip_rect(rect.expand(1.0)).rect_stroke(
                            corner,
                            radius,
                            Stroke::new(1.5, ink),
                            egui::StrokeKind::Middle,
                        );
                    }) {
                        picked = Some(Pick::Style(Restyle::Rounded(rounded)));
                    }
                }
            });
        }
        if styles.size {
            row(ui, theme, "Size", &mut |ui| {
                for (size, name) in SIZES {
                    if option(ui, theme, &format!("Size {name}"), style.size == size, |painter, rect, ink| {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            name,
                            egui::FontId::proportional(11.0),
                            ink,
                        );
                    }) {
                        picked = Some(Pick::Style(Restyle::Size(size)));
                    }
                }
            });
        }
    });
    picked
}

/// A row of a bar: its name, then its choices.
fn row(ui: &mut egui::Ui, theme: &Theme, name: &str, add: &mut dyn FnMut(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(46.0, 18.0), egui::Sense::hover());
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(11.0),
            color(theme.palette.muted),
        );
        add(ui);
    });
}

/// The arrange bar: moving the selection forward and back, and lining it
/// up. Returns what was picked.
fn arrange_bar(ui: &mut egui::Ui, theme: &Theme, styles: Styles) -> Option<Pick> {
    let mut picked = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(3.0, 4.0);
        if styles.order {
            row(ui, theme, "Order", &mut |ui| {
                for (order, name) in [
                    (Order::Back, "Send to back"),
                    (Order::Backward, "Send backward"),
                    (Order::Forward, "Bring forward"),
                    (Order::Front, "Bring to front"),
                ] {
                    if option(ui, theme, name, false, |painter, rect, ink| {
                        // An arrow down or up; to a line, for all the way.
                        let stroke = Stroke::new(1.5, ink);
                        let down = matches!(order, Order::Back | Order::Backward);
                        let (tail, tip) = if down {
                            (rect.center_top(), rect.center_bottom())
                        } else {
                            (rect.center_bottom(), rect.center_top())
                        };
                        let back = if down { -3.0 } else { 3.0 };
                        painter.line_segment([tail, tip], stroke);
                        painter.add(epaint::PathShape::line(
                            vec![tip + Vec2::new(-3.0, back), tip, tip + Vec2::new(3.0, back)],
                            stroke,
                        ));
                        if matches!(order, Order::Back | Order::Front) {
                            painter.hline(rect.center().x - 5.0..=rect.center().x + 5.0, tip.y, stroke);
                        }
                    }) {
                        picked = Some(Pick::Order(order));
                    }
                }
            });
        }
        if styles.align {
            row(ui, theme, "Align", &mut |ui| {
                for (to, name) in [
                    (Align::Left, "Align left"),
                    (Align::Center, "Align centers"),
                    (Align::Right, "Align right"),
                    (Align::Top, "Align top"),
                    (Align::Middle, "Align middles"),
                    (Align::Bottom, "Align bottom"),
                ] {
                    if option(ui, theme, name, false, |painter, rect, ink| align_picture(painter, rect, ink, to)) {
                        picked = Some(Pick::Align(to));
                    }
                }
            });
        }
    });
    picked
}

/// The picture of a way to align: the line things are lined up on, and two
/// bars lined up on it.
fn align_picture(painter: &egui::Painter, rect: egui::Rect, ink: Color32, to: Align) {
    let stroke = Stroke::new(1.0, ink);
    let upright = matches!(to, Align::Left | Align::Center | Align::Right);
    // How far across the picture the line is, and the bars' lengths.
    let at = match to {
        Align::Left | Align::Top => 0.0,
        Align::Center | Align::Middle => 0.5,
        Align::Right | Align::Bottom => 1.0,
    };
    for (index, length) in [(0, 1.0_f32), (1, 0.55)] {
        // Along the line: one bar in each half. Across it: from the line.
        let bar = if upright {
            let width = rect.width() * length;
            let left = rect.left() + (rect.width() - width) * at;
            let top = rect.top() + rect.height() * (0.12 + 0.5 * index as f32);
            egui::Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, rect.height() * 0.26))
        } else {
            let height = rect.height() * length;
            let top = rect.top() + (rect.height() - height) * at;
            let left = rect.left() + rect.width() * (0.2 + 0.4 * index as f32);
            egui::Rect::from_min_size(Pos2::new(left, top), Vec2::new(rect.width() * 0.2, height))
        };
        painter.rect_filled(bar, 1, ink);
    }
    if upright {
        let x = rect.left() + rect.width() * at;
        painter.vline(x, rect.y_range().expand(2.0), stroke);
    } else {
        let y = rect.top() + rect.height() * at;
        painter.hline(rect.x_range().expand(2.0), y, stroke);
    }
}

/// A color to pick ("Stroke Red"), or no color (`None`: "Fill None"); ringed
/// when it's the one in use. Returns whether it was clicked.
fn swatch(ui: &mut egui::Ui, theme: &Theme, of: &str, named: Option<Color>, on: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::click());
    let name = match named {
        Some(named) => format!("{of} {named:?}"),
        None => format!("{of} None"),
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, &name)
    });
    let painter = ui.painter();
    let chip = rect.shrink(3.0);
    match named {
        Some(named) => {
            painter.rect_filled(chip, 3, resolve(theme, named));
        }
        None => {
            let ink = color(theme.palette.muted);
            painter.rect_stroke(chip, 3, Stroke::new(1.0, ink), egui::StrokeKind::Inside);
            painter.line_segment([chip.left_bottom(), chip.right_top()], Stroke::new(1.0, ink));
        }
    }
    if on || response.hovered() {
        let ring = color(if on {
            theme.palette.text
        } else {
            theme.palette.muted
        });
        painter.rect_stroke(rect.shrink(0.5), 5, Stroke::new(1.5, ring), egui::StrokeKind::Inside);
    }
    response.on_hover_text(name).clicked()
}

/// One of a row's choices, with its picture drawn by `draw` in the ink it's
/// given; marked when it's the one in use. Returns whether it was clicked.
fn option(
    ui: &mut egui::Ui,
    theme: &Theme,
    name: &str,
    on: bool,
    draw: impl FnOnce(&egui::Painter, egui::Rect, Color32),
) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 18.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, name)
    });
    let palette = &theme.palette;
    let ink = color(if on {
        palette.accent
    } else if response.hovered() {
        palette.text
    } else {
        palette.muted
    });
    if on || response.hovered() {
        ui.painter().rect_filled(rect, 4, color(palette.surface));
    }
    draw(ui.painter(), rect.shrink2(Vec2::new(7.0, 4.0)), ink);
    response.on_hover_text(name).clicked()
}

/// A tool's button: its picture, drawn here (they aren't in the icon set).
fn tool_button(frame: &mut UiFrame<'_>, tool: Tool, on: bool) -> egui::Response {
    let (rect, response) = frame
        .ui
        .allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, tool.label())
    });
    let palette = &frame.theme.palette;
    let ink = color(if on {
        palette.accent
    } else if response.hovered() {
        palette.text
    } else {
        palette.muted
    });
    let painter = frame.ui.painter();
    if response.hovered() || on {
        painter.rect_filled(rect, 5, color(palette.surface));
    }
    let stroke = Stroke::new(1.5, ink);
    let art = rect.shrink(7.0);
    // A place in the picture, from 0 to 1 each way.
    let at = |x: f32, y: f32| art.min + Vec2::new(x * art.width(), y * art.height());
    match tool {
        Tool::Select => {
            painter.add(epaint::PathShape::closed_line(
                vec![
                    at(0.1, 0.0),
                    at(0.1, 0.9),
                    at(0.35, 0.67),
                    at(0.55, 1.05),
                    at(0.7, 0.97),
                    at(0.52, 0.6),
                    at(0.85, 0.6),
                ],
                stroke,
            ));
        }
        Tool::Rectangle => {
            painter.rect_stroke(
                egui::Rect::from_min_max(at(0.0, 0.12), at(1.0, 0.88)),
                1,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Tool::Ellipse => {
            painter.add(epaint::EllipseShape {
                center: art.center(),
                radius: Vec2::new(art.width() / 2.0, art.height() * 0.42),
                fill: Color32::TRANSPARENT,
                stroke,
                angle: 0.0,
            });
        }
        Tool::Diamond => {
            painter.add(epaint::PathShape::closed_line(
                vec![at(0.5, 0.0), at(1.0, 0.5), at(0.5, 1.0), at(0.0, 0.5)],
                stroke,
            ));
        }
        Tool::Line => {
            painter.line_segment([at(0.0, 1.0), at(1.0, 0.0)], stroke);
        }
        Tool::Arrow => {
            painter.line_segment([at(0.0, 1.0), at(1.0, 0.0)], stroke);
            painter.add(epaint::PathShape::line(
                vec![at(0.45, 0.0), at(1.0, 0.0), at(1.0, 0.55)],
                stroke,
            ));
        }
        Tool::Pen => {
            // A wave.
            let wave = (0..=16)
                .map(|step| {
                    let x = step as f32 / 16.0;
                    at(x, 0.5 - 0.38 * (x * std::f32::consts::TAU).sin())
                })
                .collect();
            painter.add(epaint::PathShape::line(wave, stroke));
        }
        Tool::Text => {
            painter.add(epaint::PathShape::line(
                vec![at(0.05, 0.3), at(0.05, 0.05), at(0.95, 0.05), at(0.95, 0.3)],
                stroke,
            ));
            painter.line_segment([at(0.5, 0.05), at(0.5, 1.0)], stroke);
            painter.line_segment([at(0.3, 1.0), at(0.7, 1.0)], stroke);
        }
    }
    frame.tooltip(&response, format!("{} ({})", tool.label(), tool.key()));
    response
}

fn on_screen(view: egui::Rect, camera: &Camera, rect: Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        camera.to_screen(view, Point::new(rect.x, rect.y)),
        camera.to_screen(view, Point::new(rect.right(), rect.bottom())),
    )
}

/// A named color in this theme.
pub fn resolve(theme: &Theme, named: Color) -> Color32 {
    let palette = &theme.palette;
    // The terminal's colors are the theme's hues: red, green, yellow, blue,
    // magenta and cyan at 1..=6.
    color(match named {
        Color::Ink => palette.text,
        Color::Muted => palette.muted,
        Color::Accent => palette.accent,
        Color::Red => palette.terminal[1],
        Color::Green => palette.terminal[2],
        Color::Yellow => palette.terminal[3],
        Color::Blue => palette.terminal[4],
        Color::Magenta => palette.terminal[5],
        Color::Cyan => palette.terminal[6],
    })
}

/// How much of a fill's color shows: a tint, so text over it reads.
const FILL_OPACITY: f32 = 0.22;

fn paint(
    painter: &egui::Painter,
    theme: &Theme,
    view: egui::Rect,
    camera: &Camera,
    element: &Element,
    text: bool,
) {
    let ink = resolve(theme, element.stroke);
    let stroke = Stroke::new((element.width * camera.zoom).max(0.5), ink);
    let fill = element
        .fill
        .map_or(Color32::TRANSPARENT, |fill| {
            resolve(theme, fill).gamma_multiply(FILL_OPACITY)
        });
    let place = |point: &Point| camera.to_screen(view, *point);
    let dashes = dashes(element.dash, stroke.width);
    // A dashed outline is drawn over a fill with no stroke.
    let edge = if dashes.is_some() { Stroke::NONE } else { stroke };
    match &element.shape {
        Shape::Rectangle(rect) => {
            let radius = if element.rounded {
                edit::corner_radius(rect) * camera.zoom
            } else {
                0.0
            };
            let rect = on_screen(view, camera, *rect);
            painter.rect(rect, radius, fill, edge, egui::StrokeKind::Middle);
            if dashes.is_some() {
                path(painter, rounded_corners(rect, radius), true, stroke, dashes);
            }
        }
        Shape::Ellipse(rect) => {
            let rect = on_screen(view, camera, *rect);
            painter.add(epaint::EllipseShape {
                center: rect.center(),
                radius: rect.size() / 2.0,
                fill,
                stroke: edge,
                angle: 0.0,
            });
            if dashes.is_some() {
                let around = (0..72)
                    .map(|step| {
                        let angle = step as f32 * std::f32::consts::TAU / 72.0;
                        rect.center() + rect.size() / 2.0 * Vec2::new(angle.cos(), angle.sin())
                    })
                    .collect();
                path(painter, around, true, stroke, dashes);
            }
        }
        Shape::Diamond(rect) => {
            let rect = on_screen(view, camera, *rect);
            let corners = vec![
                rect.center_top(),
                rect.right_center(),
                rect.center_bottom(),
                rect.left_center(),
            ];
            painter.add(epaint::PathShape::convex_polygon(corners.clone(), fill, edge));
            if dashes.is_some() {
                path(painter, corners, true, stroke, dashes);
            }
        }
        Shape::Line {
            points, start, end, ..
        } => {
            let points: Vec<Pos2> = points.iter().map(place).collect();
            let reach = (8.0 + 3.0 * element.width) * camera.zoom;
            if let [first, second, ..] = points[..]
                && *start == Head::Arrow
            {
                head(painter, second, first, reach, stroke);
            }
            if let [.., before, last] = points[..]
                && *end == Head::Arrow
            {
                head(painter, before, last, reach, stroke);
            }
            path(painter, points, false, stroke, dashes);
        }
        Shape::Freehand { points } => {
            // The curve through its points, made on screen: as fine as it's
            // looked at.
            let through: Vec<Point> = points
                .iter()
                .map(place)
                .map(|at| Point::new(at.x, at.y))
                .collect();
            let curve = edit::smooth(&through, 3.0)
                .into_iter()
                .map(|point| Pos2::new(point.x, point.y))
                .collect();
            path(painter, curve, false, stroke, dashes);
        }
        Shape::Text {
            at,
            text: written,
            size,
        } => {
            if let Some(font) = font(*size, camera).filter(|_| text) {
                let galley = painter.layout_no_wrap(written.clone(), font, ink);
                painter.galley(place(at), galley, ink);
            }
        }
    }
    // A box's label, in its middle.
    if text
        && element.is_box()
        && !element.label.is_empty()
        && let Some(font) = font(TEXT_SIZE, camera)
    {
        let rect = on_screen(view, camera, element.bounds());
        let galley = painter.fonts_mut(|fonts| {
            fonts.layout_job(label_job(&element.label, font, ink, label_width(rect, camera)))
        });
        painter.galley(
            Pos2::new(rect.center().x, rect.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
    }
    // A line's label, halfway along it, over the line.
    if text
        && !element.label.is_empty()
        && let Shape::Line { points, .. } = &element.shape
        && let Some(middle) = edit::midpoint(points)
        && let Some(font) = font(LINE_LABEL_SIZE, camera)
    {
        let galley = painter.fonts_mut(|fonts| {
            fonts.layout_job(label_job(&element.label, font, ink, f32::INFINITY))
        });
        let middle = camera.to_screen(view, middle);
        painter.rect_filled(
            egui::Rect::from_center_size(middle, galley.size()).expand(3.0 * camera.zoom),
            2,
            color(theme.palette.panel_background),
        );
        painter.galley(
            Pos2::new(middle.x, middle.y - galley.size().y / 2.0),
            galley,
            ink,
        );
    }
}

/// The lengths of a stroke's dashes and of the gaps between them, on screen;
/// none for a solid one.
fn dashes(dash: Dash, width: f32) -> Option<(f32, f32)> {
    match dash {
        Dash::Solid => None,
        Dash::Dashed => Some((width * 4.0 + 2.0, width * 3.0 + 1.0)),
        Dash::Dotted => Some((width, width * 2.0 + 1.0)),
    }
}

/// A stroke through the points (and back to the first, if `closed`), solid
/// or in dashes.
fn path(
    painter: &egui::Painter,
    mut points: Vec<Pos2>,
    closed: bool,
    stroke: Stroke,
    dashes: Option<(f32, f32)>,
) {
    match dashes {
        None if closed => {
            painter.add(epaint::PathShape::closed_line(points, stroke));
        }
        None => {
            painter.add(epaint::PathShape::line(points, stroke));
        }
        Some((dash, gap)) => {
            if closed && let Some(first) = points.first().copied() {
                points.push(first);
            }
            painter.extend(egui::Shape::dashed_line(&points, stroke, dash, gap));
        }
    }
}

/// The points around a rectangle with its corners rounded by `radius`.
fn rounded_corners(rect: egui::Rect, radius: f32) -> Vec<Pos2> {
    if radius < 0.5 {
        return vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    }
    let inner = rect.shrink(radius);
    let quarter = std::f32::consts::FRAC_PI_2;
    // Each corner's arc, clockwise from the top left's.
    [
        (inner.left_top(), 2.0),
        (inner.right_top(), 3.0),
        (inner.right_bottom(), 0.0),
        (inner.left_bottom(), 1.0),
    ]
    .into_iter()
    .flat_map(|(center, from)| {
        (0..=8).map(move |step| {
            let angle = (from + step as f32 / 8.0) * quarter;
            center + radius * Vec2::new(angle.cos(), angle.sin())
        })
    })
    .collect()
}

/// How high a line's label is, on the canvas.
const LINE_LABEL_SIZE: f32 = 16.0;

/// The room between a box's sides and its label, on the canvas.
const LABEL_MARGIN: f32 = 8.0;

/// How wide a label can be in a box on screen before it wraps.
fn label_width(rect: egui::Rect, camera: &Camera) -> f32 {
    (rect.width() - 2.0 * LABEL_MARGIN * camera.zoom).max(1.0)
}

/// A label laid out: wrapped to `width`, each line centered on the place
/// it's drawn at.
fn label_job(text: &str, font: egui::FontId, ink: Color32, width: f32) -> epaint::text::LayoutJob {
    let mut job = epaint::text::LayoutJob::simple(text.to_owned(), font, ink, width);
    job.halign = egui::Align::Center;
    job
}

/// The font for text `size` high on the canvas, as the camera shows it; none
/// when it's too small to read. Whole half points, so zooming doesn't lay out
/// a font per frame.
fn font(size: f32, camera: &Camera) -> Option<egui::FontId> {
    let size = (size * camera.zoom * 2.0).round() / 2.0;
    (size >= 2.0).then(|| egui::FontId::proportional(size))
}

/// The box around an element, with text measured rather than guessed at.
fn measured(fonts: &egui::Context, element: &Element) -> Rect {
    let Shape::Text { at, text, size } = &element.shape else {
        return element.bounds();
    };
    let size = fonts
        .fonts_mut(|fonts| {
            fonts.layout(
                text.clone(),
                egui::FontId::proportional(*size),
                Color32::WHITE,
                f32::INFINITY,
            )
        })
        .size();
    Rect::new(at.x, at.y, size.x, size.y)
}

/// The field for the text being written, where the text will be: a text
/// element's from its corner, a label centered in its box. Returns whether
/// the writing is done: the field lost the keyboard (a click elsewhere,
/// Escape), or Cmd/Ctrl+Enter.
fn writer(
    ui: &mut egui::Ui,
    theme: &Theme,
    view: egui::Rect,
    camera: &Camera,
    diagram: &Diagram,
    state: &mut State,
) -> bool {
    let Some(writing) = state.writing.as_mut() else {
        return false;
    };
    // What it's for: where, how large, in what color; and for a label, its
    // middle and (in a box) the width it wraps to.
    let (at, size, ink, inside) = match writing.subject {
        Subject::New(at) => (at, TEXT_SIZE, Color::Ink, None),
        Subject::Of(id) => match diagram.get(id) {
            Some(Element {
                shape: Shape::Text { at, size, .. },
                stroke,
                ..
            }) => (*at, *size, *stroke, None),
            Some(Element {
                shape: Shape::Line { points, .. },
                stroke,
                ..
            }) => {
                let middle = edit::midpoint(points).unwrap_or_default();
                (
                    middle,
                    LINE_LABEL_SIZE,
                    *stroke,
                    Some((camera.to_screen(view, middle), None)),
                )
            }
            Some(element) => {
                let rect = on_screen(view, camera, element.bounds());
                (
                    Point::default(),
                    TEXT_SIZE,
                    element.stroke,
                    Some((rect.center(), Some(label_width(rect, camera)))),
                )
            }
            // Gone (the file changed): nothing to write in.
            None => return true,
        },
    };
    let ink = resolve(theme, ink);
    let font = font(size, camera).unwrap_or(egui::FontId::proportional(2.0));
    let shown = if writing.text.is_empty() {
        " ".to_owned()
    } else {
        writing.text.clone()
    };
    let (rect, align) = match inside {
        Some((middle, wrap)) => {
            let laid_out = |width: f32| {
                ui.fonts_mut(|fonts| fonts.layout_job(label_job(&shown, font.clone(), ink, width)))
                    .size()
            };
            // A line's label is as wide as it is, with room to type into.
            let width = wrap.unwrap_or_else(|| laid_out(f32::INFINITY).x + font.size * 4.0);
            let height = laid_out(width).y;
            let rect = egui::Rect::from_center_size(middle, Vec2::new(width, height));
            if wrap.is_none() {
                // Over the line, as the label will be.
                ui.painter().rect_filled(
                    rect.expand(3.0 * camera.zoom),
                    2,
                    color(theme.palette.panel_background),
                );
            }
            (rect, egui::Align::Center)
        }
        None => {
            let size = ui
                .fonts_mut(|fonts| fonts.layout(shown, font.clone(), ink, f32::INFINITY))
                .size();
            // Room to type into, so the line doesn't wrap under the cursor.
            let room = Vec2::new(size.x + font.size * 4.0, size.y);
            (
                egui::Rect::from_min_size(camera.to_screen(view, at), room),
                egui::Align::Min,
            )
        }
    };
    let field = egui::TextEdit::multiline(&mut writing.text)
        .id(ui.id().with("bedtime.diagram.writing"))
        .font(font)
        .text_color(ink)
        .frame(egui::Frame::NONE)
        .margin(0)
        .desired_rows(1)
        .desired_width(rect.width())
        .horizontal_align(align)
        .clip_text(false);
    let response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.set_clip_rect(view);
            let response = ui.add(field);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Diagram text")
            });
            response
        })
        .inner;
    if !writing.focused {
        response.request_focus();
        writing.focused = true;
        return false;
    }
    let keep = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
    keep || response.lost_focus() || !response.has_focus()
}

/// An arrow's head at `tip`, for a line arriving from `from`: two strokes
/// `reach` long (shorter on a short line).
fn head(painter: &egui::Painter, from: Pos2, tip: Pos2, reach: f32, stroke: Stroke) {
    let along = tip - from;
    let length = along.length();
    if length < f32::EPSILON {
        return;
    }
    let back = -along / length * reach.min(length * 0.5);
    let spread = egui::emath::Rot2::from_angle(0.45);
    painter.add(epaint::PathShape::line(
        vec![tip + spread * back, tip, tip + spread.inverse() * back],
        stroke,
    ));
}

/// Dots where the grid's lines cross, so an empty canvas shows it moving.
fn grid(painter: &egui::Painter, theme: &Theme, view: egui::Rect, camera: &Camera) {
    let mut step = GRID;
    while step * camera.zoom < 20.0 {
        step *= 2.0;
    }
    let dot = color(theme.palette.muted).gamma_multiply(0.35);
    let from = camera.to_canvas(view, view.min);
    let to = camera.to_canvas(view, view.max);
    let mut y = (from.y / step).floor() * step;
    while y <= to.y {
        let mut x = (from.x / step).floor() * step;
        while x <= to.x {
            let at = camera.to_screen(view, Point::new(x, y));
            painter.rect_filled(egui::Rect::from_center_size(at, Vec2::splat(1.5)), 0, dot);
            x += step;
        }
        y += step;
    }
}
