use crate::canvas::{Camera, Edit, Pick, Request, State, Subject, ZOOM};
use crate::edit::{self, Align, Handle, Order, Restyle, Style, Tool};
use crate::model::*;
use bedtime_document::{ByteModel, DocumentModel};
use bedtime_ui::egui;

fn rectangle(id: u64) -> Element {
    Element::new(
        ElementId(id),
        Shape::Rectangle(Rect::new(id as f32 * 10.0, 0.0, 100.0, 50.0)),
    )
}

fn sample() -> Diagram {
    let mut filled = rectangle(1);
    filled.fill = Some(Color::Blue);
    let mut arrow = Element::new(
        ElementId(2),
        Shape::Line {
            points: vec![Point::new(0.0, 0.0), Point::new(40.0, 30.5)],
            start: Head::None,
            end: Head::Arrow,
            from: None,
            to: None,
            from_side: Side::Auto,
            to_side: Side::Auto,
        },
    );
    arrow.stroke = Color::Red;
    arrow.width = 4.0;
    Diagram::new(vec![
        filled,
        arrow,
        Element::new(
            ElementId(3),
            Shape::Text {
                at: Point::new(5.0, 5.0),
                text: "two\nlines, \"quoted\", it's \\' \\\\".into(),
                size: TEXT_SIZE,
            },
        ),
        Element::new(
            ElementId(4),
            Shape::Freehand {
                points: vec![
                    Point::new(1.0, 2.0),
                    Point::new(3.0, 4.0),
                    Point::new(5.0, 6.0),
                ],
            },
        ),
        Element::new(
            ElementId(5),
            Shape::Ellipse(Rect::new(0.0, 0.0, 10.0, 20.0)),
        ),
        Element::new(
            ElementId(6),
            Shape::Diamond(Rect::new(-5.0, -5.0, 10.0, 20.0)),
        ),
    ])
    .unwrap()
}

fn encoded(diagram: &Diagram) -> String {
    let mut bytes = Vec::new();
    diagram.encode(&mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn changes_return_their_inverse() {
    let original = sample();
    let mut moved = original.get(ElementId(1)).unwrap().clone();
    moved.stroke = Color::Green;
    let mut added = rectangle(9);
    added.name = "added".into();
    for change in [
        Change::Insert {
            index: 2,
            element: added,
        },
        Change::Remove { id: ElementId(2) },
        Change::Replace { element: moved },
    ] {
        let mut diagram = original.clone();
        let inverse = diagram.apply(&change).unwrap();
        assert_ne!(diagram, original, "{change:?} changed nothing");
        let again = diagram.apply(&inverse).unwrap();
        assert_eq!(diagram, original, "{change:?} wasn't undone");
        assert_eq!(again, change, "undoing {change:?} can't be redone");
    }
}

#[test]
fn removing_puts_back_in_place() {
    let mut diagram = sample();
    let inverse = diagram.apply(&Change::Remove { id: ElementId(3) }).unwrap();
    assert!(diagram.get(ElementId(3)).is_none());
    diagram.apply(&inverse).unwrap();
    assert_eq!(diagram.elements()[2].id, ElementId(3));
}

#[test]
fn changes_that_cannot_apply_are_refused() {
    let mut diagram = sample();
    let before = diagram.clone();
    assert!(
        diagram
            .apply(&Change::Insert {
                index: 0,
                element: rectangle(1)
            })
            .is_err()
    );
    assert!(
        diagram
            .apply(&Change::Insert {
                index: 99,
                element: rectangle(9)
            })
            .is_err()
    );
    assert!(
        diagram
            .apply(&Change::Remove { id: ElementId(99) })
            .is_err()
    );
    assert!(
        diagram
            .apply(&Change::Replace {
                element: rectangle(99)
            })
            .is_err()
    );
    assert_eq!(diagram, before);
}

#[test]
fn new_ids_are_above_every_other() {
    assert_eq!(Diagram::default().next_id(), ElementId(1));
    assert_eq!(sample().next_id(), ElementId(7));
}

#[test]
fn files_round_trip() {
    let diagram = sample();
    let text = encoded(&diagram);
    assert_eq!(Diagram::decode(text.as_bytes()).unwrap(), diagram, "{text}");
}

#[test]
fn files_keep_one_element_per_line() {
    let text = encoded(&sample());
    let lines: Vec<&str> = text.lines().collect();
    // "(", version, "elements: [", the six, "]," and ")".
    assert_eq!(lines.len(), 11, "{text}");
    assert_eq!(
        lines[3],
        r#"        (id: "rect-1", shape: Rectangle(x: 10, y: 0, w: 100, h: 50), fill: Blue),"#
    );
    assert!(
        lines[5].contains(r#"text: "two\nlines, \"quoted\", it's \\' \\\\""#),
        "{text}"
    );
}

#[test]
fn files_written_by_hand_are_read() {
    let diagram = Diagram::decode(
        br#"(
            version: 1,
            elements: [
                (id: "a", shape: Rectangle(x: 0, y: 0, w: 10, h: 10)),
                (shape: Line(from: (0, 0), to: (5, 5), end: Arrow), stroke: Red),
                (shape: Text(at: (1, 2), text: "hi")),
            ],
        )"#,
    )
    .unwrap();
    assert_eq!(diagram.elements().len(), 3);
    assert_eq!(diagram.elements()[0].width, STROKE_WIDTH);
    assert_eq!(diagram.elements()[1].stroke, Color::Red);
    assert!(matches!(
        diagram.elements()[2].shape,
        Shape::Text { size, .. } if size == TEXT_SIZE
    ));
}

#[test]
fn an_empty_file_is_an_empty_diagram() {
    assert!(Diagram::decode(b"").unwrap().is_empty());
    assert!(Diagram::decode(b" \n").unwrap().is_empty());
    let text = encoded(&Diagram::default());
    assert!(Diagram::decode(text.as_bytes()).unwrap().is_empty());
}

#[test]
fn files_that_are_not_diagrams_are_refused() {
    assert!(Diagram::decode(b"fn main() {}").is_err());
    assert!(Diagram::decode(&[0xff, 0xfe]).is_err());
    let later = Diagram::decode(b"(version: 2, elements: [])").unwrap_err();
    assert!(later.contains("version 2"), "{later}");
    let twice = Diagram::decode(
        b"(version: 1, elements: [
            (id: \"x\", shape: Ellipse(x: 0, y: 0, w: 1, h: 1)),
            (id: \"x\", shape: Ellipse(x: 0, y: 0, w: 1, h: 1)),
        ])",
    )
    .unwrap_err();
    assert!(twice.contains("\"x\""), "{twice}");
}

#[test]
fn bounds_hold_everything() {
    let diagram = Diagram::new(vec![
        rectangle(1),
        Element::new(
            ElementId(2),
            Shape::Line {
                points: vec![Point::new(-20.0, 80.0), Point::new(0.0, 0.0)],
                start: Head::None,
                end: Head::None,
                from: None,
                to: None,
                from_side: Side::Auto,
                to_side: Side::Auto,
            },
        ),
    ])
    .unwrap();
    assert_eq!(diagram.bounds(), Some(Rect::new(-20.0, 0.0, 130.0, 80.0)));
    assert_eq!(Diagram::default().bounds(), None);
}

fn view() -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0))
}

#[test]
fn the_camera_maps_there_and_back() {
    let camera = Camera {
        center: Point::new(30.0, -10.0),
        zoom: 2.5,
    };
    assert_eq!(camera.to_screen(view(), camera.center), view().center());
    let point = Point::new(12.0, 34.0);
    let back = camera.to_canvas(view(), camera.to_screen(view(), point));
    assert!((back.x - point.x).abs() < 1e-3 && (back.y - point.y).abs() < 1e-3);
}

#[test]
fn zooming_keeps_the_point_under_the_pointer() {
    let mut camera = Camera::default();
    let pointer = egui::pos2(250.0, 500.0);
    let held = camera.to_canvas(view(), pointer);
    camera.zoom_at(view(), pointer, 3.0);
    assert_eq!(camera.zoom, 3.0);
    let now = camera.to_screen(view(), held);
    assert!((now - pointer).length() < 1e-2, "{now:?}");
    // And stops at the limits.
    camera.zoom_at(view(), pointer, 1000.0);
    assert_eq!(camera.zoom, *ZOOM.end());
    camera.zoom_at(view(), pointer, 1e-6);
    assert_eq!(camera.zoom, *ZOOM.start());
}

#[test]
fn fitting_shows_everything_no_closer_than_actual_size() {
    // Small: centred at actual size.
    let small = Rect::new(10.0, 10.0, 100.0, 50.0);
    let camera = Camera::fitting(Some(small), view().size());
    assert_eq!(camera.zoom, 1.0);
    assert_eq!(camera.center, small.center());
    // Large: zoomed out until its corners are in view.
    let large = Rect::new(-1000.0, 0.0, 4000.0, 1000.0);
    let camera = Camera::fitting(Some(large), view().size());
    assert!(camera.zoom < 1.0);
    for corner in [Point::new(-1000.0, 0.0), Point::new(3000.0, 1000.0)] {
        assert!(view().contains(camera.to_screen(view(), corner)));
    }
    assert_eq!(Camera::fitting(None, view().size()), Camera::default());
}

// ── Editing ──────────────────────────────────────────────────────────────────

fn outline(id: u64, shape: Shape) -> Element {
    Element::new(ElementId(id), shape)
}

fn filled(id: u64, shape: Shape) -> Element {
    let mut element = outline(id, shape);
    element.fill = Some(Color::Blue);
    element
}

#[test]
fn outlines_are_picked_by_their_stroke_and_fills_anywhere() {
    let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
    let (middle, edge, outside) = (
        Point::new(50.0, 30.0),
        Point::new(0.0, 30.0),
        Point::new(-20.0, 30.0),
    );
    for shape in [
        Shape::Rectangle(rect),
        Shape::Ellipse(rect),
        Shape::Diamond(rect),
    ] {
        let hollow = outline(1, shape.clone());
        assert!(
            edit::hit(&hollow, edge, 3.0, &Element::bounds),
            "{shape:?} edge"
        );
        assert!(
            !edit::hit(&hollow, middle, 3.0, &Element::bounds),
            "{shape:?} middle"
        );
        assert!(
            !edit::hit(&hollow, outside, 3.0, &Element::bounds),
            "{shape:?} outside"
        );
        assert!(
            edit::hit(&filled(1, shape.clone()), middle, 3.0, &Element::bounds),
            "{shape:?} filled"
        );
    }
    // A box's corner is outside an ellipse and a diamond in it.
    let corner = Point::new(2.0, 2.0);
    assert!(!edit::hit(
        &filled(1, Shape::Ellipse(rect)),
        corner,
        1.0,
        &Element::bounds
    ));
    assert!(!edit::hit(
        &filled(1, Shape::Diamond(rect)),
        corner,
        1.0,
        &Element::bounds
    ));
}

#[test]
fn lines_are_picked_along_their_length() {
    let line = outline(
        1,
        Shape::Line {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(100.0, 0.0),
                Point::new(100.0, 100.0),
            ],
            start: Head::None,
            end: Head::None,
            from: None,
            to: None,
            from_side: Side::Auto,
            to_side: Side::Auto,
        },
    );
    assert!(edit::hit(
        &line,
        Point::new(50.0, 3.0),
        3.0,
        &Element::bounds
    ));
    assert!(edit::hit(
        &line,
        Point::new(98.0, 70.0),
        3.0,
        &Element::bounds
    ));
    assert!(!edit::hit(
        &line,
        Point::new(50.0, 50.0),
        3.0,
        &Element::bounds
    ));
    assert!(!edit::hit(
        &line,
        Point::new(-10.0, 0.0),
        3.0,
        &Element::bounds
    ));
}

#[test]
fn the_frontmost_element_is_picked() {
    let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
    let diagram = Diagram::new(vec![
        filled(1, Shape::Rectangle(rect)),
        filled(2, Shape::Ellipse(rect)),
    ])
    .unwrap();
    assert_eq!(
        edit::topmost_at(&diagram, rect.center(), 1.0, &Element::bounds),
        Some(ElementId(2))
    );
    // The ellipse doesn't reach the corner; the rectangle behind it does.
    assert_eq!(
        edit::topmost_at(&diagram, Point::new(2.0, 2.0), 1.0, &Element::bounds),
        Some(ElementId(1))
    );
    assert_eq!(
        edit::topmost_at(&diagram, Point::new(500.0, 0.0), 1.0, &Element::bounds),
        None
    );
}

#[test]
fn a_box_encloses_what_is_wholly_inside_it() {
    let diagram = Diagram::new(vec![
        rectangle(1),  // x 10..110
        rectangle(20), // x 200..300
    ])
    .unwrap();
    // Dragged from the bottom right: the same box.
    let area = Rect::new(150.0, 80.0, -150.0, -90.0);
    assert_eq!(
        edit::enclosed(&diagram, area, &Element::bounds),
        vec![ElementId(1)]
    );
    assert_eq!(
        edit::enclosed(
            &diagram,
            Rect::new(0.0, -10.0, 250.0, 100.0),
            &Element::bounds
        ),
        vec![ElementId(1)],
        "half of the second isn't enough"
    );
}

#[test]
fn tools_make_their_shapes() {
    let (from, to) = (Point::new(100.0, 100.0), Point::new(40.0, 130.0));
    let made = edit::create(Tool::Ellipse, ElementId(7), from, to, false).unwrap();
    assert_eq!(made.id, ElementId(7));
    assert_eq!(
        made.shape,
        Shape::Ellipse(Rect::new(40.0, 100.0, 60.0, 30.0))
    );
    // Constrained: square, still from where the drag began.
    let square = edit::create(Tool::Rectangle, ElementId(1), from, to, true).unwrap();
    assert_eq!(
        square.shape,
        Shape::Rectangle(Rect::new(40.0, 100.0, 60.0, 60.0))
    );
    let arrow = edit::create(Tool::Arrow, ElementId(1), from, to, false).unwrap();
    assert_eq!(
        arrow.shape,
        Shape::Line {
            points: vec![from, to],
            start: Head::None,
            end: Head::Arrow,
            from: None,
            to: None,
            from_side: Side::Auto,
            to_side: Side::Auto,
        }
    );
    // Constrained: the nearest 15 degrees, here straight left.
    let Shape::Line { points, .. } =
        edit::create(Tool::Line, ElementId(1), from, Point::new(0.0, 104.0), true)
            .unwrap()
            .shape
    else {
        panic!("not a line");
    };
    assert!((points[1].y - 100.0).abs() < 1e-3, "{points:?}");
    // A click makes nothing, and neither does the Select tool.
    assert!(
        edit::create(
            Tool::Rectangle,
            ElementId(1),
            from,
            Point::new(101.0, 102.0),
            false
        )
        .is_none()
    );
    assert!(edit::create(Tool::Select, ElementId(1), from, to, false).is_none());
    assert!(edit::create(Tool::Text, ElementId(1), from, to, false).is_none());
    assert!(edit::create(Tool::Pen, ElementId(1), from, to, false).is_none());
}

#[test]
fn moving_moves_every_kind() {
    for element in sample().elements() {
        let moved = edit::translate(element, 5.0, -7.0);
        let (before, after) = (element.bounds(), moved.bounds());
        assert_eq!(
            (after.x, after.y),
            (before.x + 5.0, before.y - 7.0),
            "{element:?}"
        );
        assert_eq!((after.w, after.h), (before.w, before.h), "{element:?}");
        assert_eq!(edit::translate(&moved, -5.0, 7.0), *element);
    }
}

#[test]
fn handles_stretch_and_mirror() {
    let from = Rect::new(0.0, 0.0, 100.0, 50.0);
    let corner = Handle { x: 1, y: 1 };
    assert_eq!(
        corner.drag(from, Point::new(200.0, 80.0)),
        Rect::new(0.0, 0.0, 200.0, 80.0)
    );
    // A side's handle leaves the other way alone.
    let side = Handle { x: -1, y: 0 };
    assert_eq!(
        side.drag(from, Point::new(-50.0, 999.0)),
        Rect::new(-50.0, 0.0, 150.0, 50.0)
    );

    let to = corner.drag(from, Point::new(200.0, 100.0));
    let boxed = edit::remap(
        &outline(1, Shape::Rectangle(Rect::new(50.0, 25.0, 50.0, 25.0))),
        from,
        to,
    );
    assert_eq!(
        boxed.shape,
        Shape::Rectangle(Rect::new(100.0, 50.0, 100.0, 50.0))
    );

    // Past the other side: mirrored, and a box is still a box.
    let flipped = corner.drag(from, Point::new(-100.0, 50.0));
    let boxed = edit::remap(
        &outline(1, Shape::Rectangle(Rect::new(0.0, 0.0, 25.0, 50.0))),
        from,
        flipped,
    );
    assert_eq!(
        boxed.shape,
        Shape::Rectangle(Rect::new(-25.0, 0.0, 25.0, 50.0))
    );
    let line = outline(
        1,
        Shape::Line {
            points: vec![Point::new(0.0, 0.0), Point::new(100.0, 50.0)],
            start: Head::None,
            end: Head::Arrow,
            from: None,
            to: None,
            from_side: Side::Auto,
            to_side: Side::Auto,
        },
    );
    let Shape::Line { points, .. } = edit::remap(&line, from, flipped).shape else {
        panic!("not a line");
    };
    assert_eq!(points, vec![Point::new(0.0, 0.0), Point::new(-100.0, 50.0)]);
}

#[test]
fn a_flat_line_moves_rather_than_scaling_by_nothing() {
    let flat = outline(
        1,
        Shape::Line {
            points: vec![Point::new(0.0, 10.0), Point::new(100.0, 10.0)],
            start: Head::None,
            end: Head::None,
            from: None,
            to: None,
            from_side: Side::Auto,
            to_side: Side::Auto,
        },
    );
    let from = flat.bounds();
    let resized = edit::remap(&flat, from, Rect::new(0.0, 30.0, 50.0, 0.0));
    assert_eq!(resized.bounds(), Rect::new(0.0, 30.0, 50.0, 0.0));
}

// ── Gestures ─────────────────────────────────────────────────────────────────

/// A canvas where a canvas point is the same point on screen.
fn plain() -> (egui::Rect, Camera) {
    let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let camera = Camera {
        center: Point::new(400.0, 300.0),
        zoom: 1.0,
    };
    (view, camera)
}

fn applied(diagram: &Diagram, edit: &Edit) -> Diagram {
    let mut diagram = diagram.clone();
    for change in &edit.changes {
        diagram.apply(change).unwrap();
    }
    diagram
}

fn two_boxes() -> Diagram {
    Diagram::new(vec![
        filled(1, Shape::Rectangle(Rect::new(100.0, 100.0, 100.0, 60.0))),
        filled(2, Shape::Rectangle(Rect::new(300.0, 100.0, 100.0, 60.0))),
    ])
    .unwrap()
}

#[test]
fn dragging_with_a_tool_adds_an_element_and_selects_it() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.tool = Tool::Diamond;
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(500.0, 300.0),
            egui::pos2(600.0, 380.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Add Diamond");
    let after = applied(&diagram, &edit);
    let made = after.elements().last().unwrap();
    assert_eq!(made.id, ElementId(3));
    assert_eq!(
        made.shape,
        Shape::Diamond(Rect::new(500.0, 300.0, 100.0, 80.0))
    );
    assert_eq!(state.selection, vec![ElementId(3)]);
    assert_eq!(state.tool, Tool::Select, "the tool is put down");
    assert!(!state.is_busy());

    // A drag too short to be a shape adds nothing, and keeps the tool.
    state.tool = Tool::Rectangle;
    let none = state.drag(
        &diagram,
        view,
        &mut camera,
        egui::pos2(10.0, 10.0),
        egui::pos2(11.0, 12.0),
        false,
    );
    assert_eq!(none, None);
    assert_eq!(state.tool, Tool::Rectangle);
}

#[test]
fn dragging_an_element_selects_and_moves_it() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(170.0, 180.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Move");
    assert_eq!(state.selection, vec![ElementId(1)]);
    let after = applied(&diagram, &edit);
    assert_eq!(
        after.get(ElementId(1)).unwrap().shape,
        Shape::Rectangle(Rect::new(120.0, 150.0, 100.0, 60.0))
    );
    assert_eq!(
        after.get(ElementId(2)),
        diagram.get(ElementId(2)),
        "the other stays"
    );
}

#[test]
fn dragging_over_nothing_selects_what_the_box_encloses() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    let edit = state.drag(
        &diagram,
        view,
        &mut camera,
        egui::pos2(50.0, 50.0),
        egui::pos2(450.0, 200.0),
        false,
    );
    assert_eq!(edit, None);
    assert_eq!(state.selection, vec![ElementId(1), ElementId(2)]);

    // Both move together, dragged by either.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(350.0, 130.0),
            egui::pos2(350.0, 230.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.changes.len(), 2);
    let after = applied(&diagram, &edit);
    assert_eq!(after.bounds(), Some(Rect::new(100.0, 200.0, 300.0, 60.0)));

    // A box around one, with Shift, adds to what's selected; without, replaces it.
    state.selection = vec![ElementId(2)];
    state.drag(
        &diagram,
        view,
        &mut camera,
        egui::pos2(50.0, 50.0),
        egui::pos2(250.0, 200.0),
        true,
    );
    assert_eq!(state.selection, vec![ElementId(2), ElementId(1)]);
    state.drag(
        &diagram,
        view,
        &mut camera,
        egui::pos2(50.0, 50.0),
        egui::pos2(250.0, 200.0),
        false,
    );
    assert_eq!(state.selection, vec![ElementId(1)]);
    state.drag(
        &diagram,
        view,
        &mut camera,
        egui::pos2(500.0, 400.0),
        egui::pos2(600.0, 500.0),
        false,
    );
    assert!(state.selection.is_empty());
}

#[test]
fn dragging_a_corner_resizes_the_selection() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.selection = vec![ElementId(1)];
    // The frame's corner is a little outside the box's (200, 160).
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(205.0, 165.0),
            egui::pos2(305.0, 225.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Resize");
    let after = applied(&diagram, &edit);
    assert_eq!(
        after.get(ElementId(1)).unwrap().shape,
        Shape::Rectangle(Rect::new(100.0, 100.0, 200.0, 120.0))
    );
}

#[test]
fn dragging_a_line_s_end_moves_that_point() {
    let (view, mut camera) = plain();
    let line = outline(
        1,
        Shape::Line {
            points: vec![Point::new(100.0, 100.0), Point::new(300.0, 100.0)],
            start: Head::None,
            end: Head::Arrow,
            from: None,
            to: None,
            from_side: Side::Auto,
            to_side: Side::Auto,
        },
    );
    let diagram = Diagram::new(vec![line]).unwrap();
    let mut state = State::default();
    state.selection = vec![ElementId(1)];
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(300.0, 100.0),
            egui::pos2(320.0, 240.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Move Point");
    let after = applied(&diagram, &edit);
    let Shape::Line { points, .. } = &after.get(ElementId(1)).unwrap().shape else {
        panic!("not a line");
    };
    assert_eq!(
        *points,
        vec![Point::new(100.0, 100.0), Point::new(320.0, 240.0)]
    );
}

#[test]
fn a_drag_that_goes_nowhere_makes_no_edit() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    let at = egui::pos2(150.0, 130.0);
    assert_eq!(state.drag(&diagram, view, &mut camera, at, at, false), None);
    assert_eq!(state.selection, vec![ElementId(1)]);
}

// ── Text ─────────────────────────────────────────────────────────────────────

fn text(id: u64, at: Point, text: &str) -> Element {
    outline(
        id,
        Shape::Text {
            at,
            text: text.into(),
            size: TEXT_SIZE,
        },
    )
}

/// What `state` is writing becomes `text`, and is kept.
fn wrote(state: &mut State, diagram: &Diagram, text: &str) -> Option<Edit> {
    state.writing.as_mut().expect("writing").text = text.into();
    state.written(diagram)
}

#[test]
fn labels_are_kept_in_the_file() {
    let mut labelled = rectangle(1);
    labelled.label = "Open\nfile".into();
    let diagram = Diagram::new(vec![labelled, rectangle(2)]).unwrap();
    let written = encoded(&diagram);
    assert!(
        written.contains(r#"h: 50), label: "Open\nfile"),"#),
        "{written}"
    );
    assert_eq!(
        written.matches("label").count(),
        1,
        "no label isn't written"
    );
    assert_eq!(Diagram::decode(written.as_bytes()).unwrap(), diagram);
}

#[test]
fn clicking_with_the_text_tool_writes_new_text() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.tool = Tool::Text;
    let at = egui::pos2(500.0, 400.0);
    assert_eq!(state.drag(&diagram, view, &mut camera, at, at, false), None);
    assert!(state.is_writing() && !state.is_busy());

    let edit = wrote(&mut state, &diagram, "hello\nthere\n").unwrap();
    assert_eq!(edit.label, "Add Text");
    let after = applied(&diagram, &edit);
    let made = after.elements().last().unwrap();
    let Shape::Text { at, text, .. } = &made.shape else {
        panic!("not text");
    };
    assert_eq!(text, "hello\nthere", "without the last line's end");
    assert_eq!(at.x, 500.0);
    assert!(
        at.y < 400.0 && at.y > 400.0 - TEXT_SIZE,
        "the click is in the first line"
    );
    assert_eq!(state.selection, vec![made.id]);
    assert_eq!(state.tool, Tool::Select);
    assert!(!state.is_writing());
}

#[test]
fn new_text_left_empty_is_nothing() {
    let diagram = two_boxes();
    let mut state = State::default();
    state.tool = Tool::Text;
    state.write(&diagram, Subject::New(Point::new(0.0, 0.0)));
    assert_eq!(wrote(&mut state, &diagram, "  \n"), None);
    assert!(!state.is_writing());
    assert_eq!(state.tool, Tool::Select);
}

#[test]
fn writing_in_a_box_sets_its_label() {
    let diagram = two_boxes();
    let mut state = State::default();
    state.write(&diagram, Subject::Of(ElementId(1)));
    let edit = wrote(&mut state, &diagram, "Start").unwrap();
    assert_eq!(edit.label, "Edit Text");
    let after = applied(&diagram, &edit);
    assert_eq!(after.get(ElementId(1)).unwrap().label, "Start");
    assert_eq!(
        after.get(ElementId(1)).unwrap().shape,
        diagram.get(ElementId(1)).unwrap().shape
    );

    // Writing again starts from the label; the same text changes nothing,
    // and none takes the label away.
    state.write(&after, Subject::Of(ElementId(1)));
    assert_eq!(state.writing.as_ref().unwrap().text, "Start");
    assert_eq!(state.written(&after), None);
    state.write(&after, Subject::Of(ElementId(1)));
    let cleared = wrote(&mut state, &after, "").unwrap();
    assert_eq!(applied(&after, &cleared), diagram);
}

#[test]
fn writing_in_text_changes_it_and_emptying_it_removes_it() {
    let diagram = Diagram::new(vec![
        rectangle(1),
        text(2, Point::new(0.0, 100.0), "before"),
    ])
    .unwrap();
    let mut state = State::default();
    state.selection = vec![ElementId(2)];
    state.write(&diagram, Subject::Of(ElementId(2)));
    assert_eq!(state.writing.as_ref().unwrap().text, "before");
    let edit = wrote(&mut state, &diagram, "after").unwrap();
    let after = applied(&diagram, &edit);
    assert!(matches!(
        &after.get(ElementId(2)).unwrap().shape,
        Shape::Text { text, .. } if text == "after"
    ));

    state.write(&after, Subject::Of(ElementId(2)));
    let edit = wrote(&mut state, &after, " ").unwrap();
    assert_eq!(edit.changes, vec![Change::Remove { id: ElementId(2) }]);
    assert!(state.selection.is_empty());
}

#[test]
fn a_line_s_label_is_written_on_it_and_a_stroke_has_none() {
    let stroke = outline(
        2,
        Shape::Freehand {
            points: vec![Point::new(0.0, 50.0), Point::new(100.0, 50.0)],
        },
    );
    let diagram = Diagram::new(vec![line(1, (0.0, 0.0), (100.0, 0.0)), stroke]).unwrap();
    let at = |x, y| edit::writable_at(&diagram, Point::new(x, y), 3.0, &Element::bounds);
    assert_eq!(at(50.0, 2.0), Some(ElementId(1)));
    assert_eq!(at(50.0, 20.0), None);
    assert_eq!(at(50.0, 50.0), None, "a stroke takes no label");

    let mut state = State::default();
    state.write(&diagram, Subject::Of(ElementId(2)));
    assert!(!state.is_writing());
    state.write(&diagram, Subject::Of(ElementId(1)));
    let edit = wrote(&mut state, &diagram, "yes").unwrap();
    assert_eq!(
        applied(&diagram, &edit).get(ElementId(1)).unwrap().label,
        "yes"
    );
}

#[test]
fn a_box_is_written_in_anywhere_inside_it() {
    let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
    let diagram = Diagram::new(vec![
        outline(1, Shape::Rectangle(rect)),
        outline(2, Shape::Ellipse(Rect::new(200.0, 0.0, 100.0, 60.0))),
        text(3, Point::new(0.0, 100.0), "words"),
    ])
    .unwrap();
    let at = |x, y| edit::writable_at(&diagram, Point::new(x, y), 3.0, &Element::bounds);
    // An outline's middle isn't picked by a click, but it's where its label goes.
    assert_eq!(
        edit::topmost_at(&diagram, rect.center(), 3.0, &Element::bounds),
        None
    );
    assert_eq!(at(50.0, 30.0), Some(ElementId(1)));
    assert_eq!(at(250.0, 30.0), Some(ElementId(2)));
    assert_eq!(at(202.0, 2.0), None, "outside the ellipse, in its box");
    assert_eq!(at(10.0, 110.0), Some(ElementId(3)));
    assert_eq!(at(150.0, 30.0), None);
}

// ── Binding ──────────────────────────────────────────────────────────────────

/// A plain line between two points.
fn line(id: u64, from: (f32, f32), to: (f32, f32)) -> Element {
    outline(
        id,
        Shape::Line {
            points: vec![Point::new(from.0, from.1), Point::new(to.0, to.1)],
            start: Head::None,
            end: Head::Arrow,
            from: None,
            to: None,
            from_side: Side::Auto,
            to_side: Side::Auto,
        },
    )
}

/// A line's points and what its ends are bound to.
fn ends(diagram: &Diagram, id: u64) -> (Vec<Point>, Option<ElementId>, Option<ElementId>) {
    match &diagram.get(ElementId(id)).unwrap().shape {
        Shape::Line {
            points, from, to, ..
        } => (points.clone(), *from, *to),
        other => panic!("not a line: {other:?}"),
    }
}

fn near(a: Point, b: (f32, f32)) -> bool {
    (a.x - b.0).abs() < 1e-2 && (a.y - b.1).abs() < 1e-2
}

/// Two boxes side by side (x 100..200 and 300..400, y 100..160) and an arrow
/// drawn from the middle of one to the middle of the other: pinned to no side.
fn joined() -> Diagram {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.tool = Tool::Arrow;
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(350.0, 130.0),
            false,
        )
        .unwrap();
    applied(&diagram, &edit)
}

#[test]
fn a_line_meets_each_outline_on_the_way_from_its_middle() {
    let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
    let gap = edit::BIND_GAP + STROKE_WIDTH / 2.0;
    for (shape, right, below) in [
        (Shape::Rectangle(rect), 100.0, 60.0),
        (Shape::Ellipse(rect), 100.0, 60.0),
        (Shape::Diamond(rect), 100.0, 60.0),
    ] {
        let target = outline(1, shape.clone());
        let at = edit::attach(&target, Point::new(500.0, 30.0), &Element::bounds);
        assert!(near(at, (right + gap, 30.0)), "{shape:?} {at:?}");
        let at = edit::attach(&target, Point::new(50.0, 900.0), &Element::bounds);
        assert!(near(at, (50.0, below + gap)), "{shape:?} {at:?}");
        // Wherever it comes from, it ends outside the outline.
        let at = edit::attach(&target, Point::new(400.0, 300.0), &Element::bounds);
        assert!(
            !edit::hit(&filled(1, shape.clone()), at, 0.0, &Element::bounds),
            "{shape:?}"
        );
    }
    // A corner: a rectangle's is further out than an ellipse's.
    let toward = Point::new(150.0, 90.0);
    let square = edit::attach(
        &outline(1, Shape::Rectangle(rect)),
        toward,
        &Element::bounds,
    );
    let round = edit::attach(&outline(1, Shape::Ellipse(rect)), toward, &Element::bounds);
    assert!(square.x > round.x && square.y > round.y);
}

#[test]
fn an_arrow_drawn_between_boxes_is_bound_to_both() {
    let diagram = joined();
    let (points, from, to) = ends(&diagram, 3);
    assert_eq!((from, to), (Some(ElementId(1)), Some(ElementId(2))));
    // Middle to middle, stopping short of each outline.
    let gap = edit::BIND_GAP + STROKE_WIDTH / 2.0;
    assert!(near(points[0], (200.0 + gap, 130.0)), "{points:?}");
    assert!(near(points[1], (300.0 - gap, 130.0)), "{points:?}");
}

#[test]
fn an_arrow_drawn_over_nothing_is_bound_to_nothing() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.tool = Tool::Arrow;
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(100.0, 300.0),
            egui::pos2(300.0, 400.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    assert_eq!(
        ends(&after, 3),
        (
            vec![Point::new(100.0, 300.0), Point::new(300.0, 400.0)],
            None,
            None
        )
    );
    // And one that starts and ends in the same box is bound by its start only.
    state.tool = Tool::Arrow;
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(110.0, 110.0),
            egui::pos2(190.0, 150.0),
            false,
        )
        .unwrap();
    let (_, from, to) = ends(&applied(&diagram, &edit), 3);
    assert_eq!((from, to), (Some(ElementId(1)), None));
}

#[test]
fn a_bound_arrow_follows_what_it_is_bound_to() {
    let (view, mut camera) = plain();
    let diagram = joined();
    let mut state = State::default();
    // The second box, dragged straight down by 200.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(350.0, 130.0),
            egui::pos2(350.0, 330.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Move");
    assert_eq!(edit.changes.len(), 2, "the box, and the arrow after it");
    let after = applied(&diagram, &edit);
    let (points, from, to) = ends(&after, 3);
    assert_eq!((from, to), (Some(ElementId(1)), Some(ElementId(2))));
    // Still middle to middle: now down and to the right.
    let (a, b) = (Point::new(150.0, 130.0), Point::new(350.0, 330.0));
    for point in &points {
        let cross = (b.x - a.x) * (point.y - a.y) - (b.y - a.y) * (point.x - a.x);
        assert!(
            cross.abs() < 1.0,
            "{point:?} is off the way between the middles"
        );
    }
    assert!(
        points[0].y > 130.0 && points[1].y < 330.0 && points[1].x < 350.0,
        "{points:?}"
    );

    // Resizing what it's bound to moves it too.
    state.selection = vec![ElementId(2)];
    let before = ends(&after, 3).0;
    let edit = state
        .drag(
            &after,
            view,
            &mut camera,
            egui::pos2(295.0, 295.0),
            egui::pos2(195.0, 295.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Resize");
    assert_ne!(ends(&applied(&after, &edit), 3).0, before);
}

#[test]
fn a_bound_arrow_moved_with_its_boxes_stays_bound_and_alone_lets_go() {
    let (view, mut camera) = plain();
    let diagram = joined();
    let mut state = State::default();
    state.selection = vec![ElementId(1), ElementId(2), ElementId(3)];
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(160.0, 180.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    let (before, ..) = ends(&diagram, 3);
    let (points, from, to) = ends(&after, 3);
    assert_eq!((from, to), (Some(ElementId(1)), Some(ElementId(2))));
    assert!(
        near(points[0], (before[0].x + 10.0, before[0].y + 50.0)),
        "{points:?}"
    );

    // By itself (dragged by other than its middle, which would bend it), it
    // comes away from both.
    state.selection = vec![ElementId(3)];
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(230.0, 130.0),
            egui::pos2(230.0, 230.0),
            false,
        )
        .unwrap();
    let (points, from, to) = ends(&applied(&diagram, &edit), 3);
    assert_eq!((from, to), (None, None));
    assert!(
        near(points[0], (before[0].x, before[0].y + 100.0)),
        "{points:?}"
    );
}

#[test]
fn dragging_an_end_binds_it_to_what_it_is_dropped_on() {
    let (view, mut camera) = plain();
    let mut elements = two_boxes().elements().to_vec();
    elements.push(line(3, (150.0, 300.0), (250.0, 300.0)));
    let diagram = Diagram::new(elements).unwrap();
    let mut state = State::default();
    state.selection = vec![ElementId(3)];
    // Its end, onto the second box.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(250.0, 300.0),
            egui::pos2(350.0, 140.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    let (points, from, to) = ends(&after, 3);
    assert_eq!((from, to), (None, Some(ElementId(2))));
    assert_eq!(points[0], Point::new(150.0, 300.0), "the other end stays");
    assert!(
        points[1].y > 160.0 && points[1].y < 170.0,
        "under the box: {points:?}"
    );

    // And off it again, onto nothing.
    let end = egui::pos2(points[1].x, points[1].y);
    let edit = state
        .drag(
            &after,
            view,
            &mut camera,
            end,
            egui::pos2(500.0, 500.0),
            false,
        )
        .unwrap();
    assert_eq!(
        ends(&applied(&after, &edit), 3),
        (
            vec![Point::new(150.0, 300.0), Point::new(500.0, 500.0)],
            None,
            None
        )
    );
}

#[test]
fn deleting_what_an_arrow_is_bound_to_lets_it_go() {
    let diagram = joined();
    let before = ends(&diagram, 3).0;
    let changes = edit::with_bindings(
        &diagram,
        vec![Change::Remove { id: ElementId(2) }],
        &Element::bounds,
    );
    assert_eq!(changes.len(), 2);
    let mut after = diagram.clone();
    let mut inverses = Vec::new();
    for change in &changes {
        inverses.push(after.apply(change).unwrap());
    }
    let (points, from, to) = ends(&after, 3);
    assert_eq!((from, to), (Some(ElementId(1)), None));
    assert_eq!(points[1], before[1], "the loose end stays where it was");
    // Undone, back to front, it's as it was: bound again.
    for inverse in inverses.iter().rev() {
        after.apply(inverse).unwrap();
    }
    assert_eq!(after, diagram);
}

#[test]
fn bindings_are_kept_in_the_file() {
    let diagram = joined();
    let written = encoded(&diagram);
    assert!(
        written.contains(r#"Arrow(from: "rect-1", to: "rect-2")"#),
        "{written}"
    );
    assert_eq!(Diagram::decode(written.as_bytes()).unwrap(), diagram);
    // A line bound to something that isn't there is let go when settled.
    let mut loose = line(1, (0.0, 0.0), (9.0, 9.0));
    edit::bind_end(&mut loose, 1, Some((ElementId(7), Side::Left)));
    let loose = Diagram::new(vec![loose]).unwrap();
    let settled = edit::settle(&loose.elements()[0], &loose, &Element::bounds);
    assert_eq!(ends(&Diagram::new(vec![settled]).unwrap(), 1).2, None);
}

#[test]
fn halfway_along_a_line_is_by_its_length() {
    let bent = [
        Point::new(0.0, 0.0),
        Point::new(10.0, 0.0),
        Point::new(10.0, 30.0),
    ];
    assert_eq!(edit::midpoint(&bent), Some(Point::new(10.0, 10.0)));
    assert_eq!(edit::midpoint(&bent[..1]), Some(Point::new(0.0, 0.0)));
    assert_eq!(edit::midpoint(&[]), None);
}

// ── Pinning to a side ────────────────────────────────────────────────────────

/// The sides a line's ends are pinned to.
fn sides(diagram: &Diagram, id: u64) -> (Side, Side) {
    match &diagram.get(ElementId(id)).unwrap().shape {
        Shape::Line {
            from_side, to_side, ..
        } => (*from_side, *to_side),
        other => panic!("not a line: {other:?}"),
    }
}

#[test]
fn an_end_is_pinned_to_the_side_it_is_dropped_near() {
    let target = outline(1, Shape::Rectangle(Rect::new(0.0, 0.0, 100.0, 60.0)));
    let side = |x, y| edit::side_at(&target, Point::new(x, y), &Element::bounds);
    assert_eq!(side(50.0, 30.0), Side::Auto);
    assert_eq!(side(60.0, 38.0), Side::Auto, "still its middle");
    assert_eq!(side(95.0, 30.0), Side::Right);
    assert_eq!(side(104.0, 40.0), Side::Right, "just outside it");
    assert_eq!(side(3.0, 20.0), Side::Left);
    assert_eq!(side(50.0, 2.0), Side::Top);
    assert_eq!(side(40.0, 58.0), Side::Bottom);
}

#[test]
fn a_pinned_end_is_in_the_middle_of_its_side() {
    let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
    let gap = edit::BIND_GAP + STROKE_WIDTH / 2.0;
    for shape in [
        Shape::Rectangle(rect),
        Shape::Ellipse(rect),
        Shape::Diamond(rect),
    ] {
        let target = outline(1, shape);
        let at = |side| edit::attach_side(&target, side, &Element::bounds);
        assert_eq!(at(Side::Top), Some(Point::new(50.0, -gap)));
        assert_eq!(at(Side::Right), Some(Point::new(100.0 + gap, 30.0)));
        assert_eq!(at(Side::Bottom), Some(Point::new(50.0, 60.0 + gap)));
        assert_eq!(at(Side::Left), Some(Point::new(-gap, 30.0)));
        assert_eq!(at(Side::Auto), None);
    }
}

#[test]
fn an_arrow_drawn_from_side_to_side_stays_on_those_sides() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.tool = Tool::Arrow;
    // From the bottom of the first box to the top of the second.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 158.0),
            egui::pos2(350.0, 102.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    assert_eq!(sides(&after, 3), (Side::Bottom, Side::Top));
    let gap = edit::BIND_GAP + STROKE_WIDTH / 2.0;
    let (points, ..) = ends(&after, 3);
    assert_eq!(
        points,
        vec![
            Point::new(150.0, 160.0 + gap),
            Point::new(350.0, 100.0 - gap)
        ]
    );

    // The second box, moved under the first: its end is still on top.
    state.selection.clear();
    let edit = state
        .drag(
            &after,
            view,
            &mut camera,
            egui::pos2(350.0, 130.0),
            egui::pos2(150.0, 430.0),
            false,
        )
        .unwrap();
    let moved = applied(&after, &edit);
    assert_eq!(sides(&moved, 3), (Side::Bottom, Side::Top));
    let (points, ..) = ends(&moved, 3);
    assert_eq!(
        points,
        vec![
            Point::new(150.0, 160.0 + gap),
            Point::new(150.0, 400.0 - gap)
        ]
    );
}

#[test]
fn an_end_not_pinned_faces_the_other_s_pinned_end() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.tool = Tool::Arrow;
    // From the first box's middle to the top of the second.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(350.0, 102.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    assert_eq!(sides(&after, 3), (Side::Auto, Side::Top));
    let (points, ..) = ends(&after, 3);
    // It leaves the first box upwards of level: towards the second's top.
    assert!(points[0].x > 200.0 && points[0].y < 130.0, "{points:?}");
}

#[test]
fn dragging_an_end_pins_it_anew_and_letting_go_forgets_the_side() {
    let (view, mut camera) = plain();
    let diagram = joined();
    let mut state = State::default();
    state.selection = vec![ElementId(3)];
    let (points, ..) = ends(&diagram, 3);
    // Its end, from the second box's left to near that box's bottom.
    let end = egui::pos2(points[1].x, points[1].y);
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            end,
            egui::pos2(350.0, 157.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    assert_eq!(sides(&after, 3), (Side::Auto, Side::Bottom));
    let written = encoded(&after);
    assert!(
        written.contains(r#"Arrow(from: "rect-1", to: "rect-2", to_side: Bottom)"#),
        "{written}"
    );
    assert_eq!(Diagram::decode(written.as_bytes()).unwrap(), after);

    // Then off it, onto nothing: no side is left behind.
    let (points, ..) = ends(&after, 3);
    let end = egui::pos2(points[1].x, points[1].y);
    let edit = state
        .drag(
            &after,
            view,
            &mut camera,
            end,
            egui::pos2(600.0, 500.0),
            false,
        )
        .unwrap();
    let loose = applied(&after, &edit);
    assert_eq!(ends(&loose, 3).2, None);
    assert_eq!(sides(&loose, 3), (Side::Auto, Side::Auto));
}

// ── Style ────────────────────────────────────────────────────────────────────

#[test]
fn a_style_goes_to_what_takes_it() {
    let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
    let boxed = outline(1, Shape::Rectangle(rect));
    let round = outline(2, Shape::Ellipse(rect));
    let arrow = line(3, (0.0, 0.0), (10.0, 10.0));
    let words = text(4, Point::new(0.0, 0.0), "words");
    let all = [&boxed, &round, &arrow, &words];
    let changed = |change: Restyle| -> Vec<u64> {
        all.iter()
            .filter(|element| edit::restyle(element, change) != ***element)
            .map(|element| element.id.0)
            .collect()
    };
    assert_eq!(changed(Restyle::Stroke(Color::Red)), vec![1, 2, 3, 4]);
    assert_eq!(changed(Restyle::Fill(Some(Color::Blue))), vec![1, 2]);
    assert_eq!(changed(Restyle::Width(4.0)), vec![1, 2, 3]);
    assert_eq!(changed(Restyle::Dash(Dash::Dashed)), vec![1, 2, 3]);
    assert_eq!(changed(Restyle::Rounded(true)), vec![1]);
    assert_eq!(changed(Restyle::Size(36.0)), vec![4]);
    // And what's already so isn't changed.
    assert_eq!(changed(Restyle::Stroke(Color::Ink)), Vec::<u64>::new());
    assert!(matches!(
        edit::restyle(&words, Restyle::Size(36.0)).shape,
        Shape::Text { size, .. } if size == 36.0
    ));
}

#[test]
fn picking_a_style_restyles_the_selection_in_one_step() {
    let mut elements = two_boxes().elements().to_vec();
    elements.push(line(3, (0.0, 300.0), (100.0, 300.0)));
    let diagram = Diagram::new(elements).unwrap();
    let mut state = State::default();
    state.selection = vec![ElementId(2), ElementId(3)];
    let edit = state.restyle(&diagram, Restyle::Fill(None)).unwrap();
    assert_eq!(edit.label, "Style");
    assert_eq!(edit.changes.len(), 1, "the line has no fill to change");
    let after = applied(&diagram, &edit);
    assert_eq!(after.get(ElementId(2)).unwrap().fill, None);
    assert_eq!(
        after.get(ElementId(1)),
        diagram.get(ElementId(1)),
        "not selected"
    );

    let edit = state.restyle(&after, Restyle::Dash(Dash::Dotted)).unwrap();
    assert_eq!(edit.changes.len(), 2);
    // Picking what's already so, or with nothing selected, changes nothing.
    assert_eq!(state.restyle(&after, Restyle::Fill(None)), None);
    state.selection.clear();
    assert_eq!(state.restyle(&after, Restyle::Stroke(Color::Red)), None);
}

#[test]
fn what_is_made_next_is_in_the_style_last_picked() {
    let (view, mut camera) = plain();
    let diagram = Diagram::default();
    let mut state = State::default();
    for change in [
        Restyle::Stroke(Color::Green),
        Restyle::Fill(Some(Color::Yellow)),
        Restyle::Width(4.0),
        Restyle::Dash(Dash::Dashed),
        Restyle::Rounded(true),
        Restyle::Size(28.0),
    ] {
        assert_eq!(state.restyle(&diagram, change), None);
    }
    state.tool = Tool::Rectangle;
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(100.0, 100.0),
            egui::pos2(200.0, 160.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    let made = &after.elements()[0];
    assert_eq!(
        (made.stroke, made.fill, made.width, made.dash, made.rounded),
        (Color::Green, Some(Color::Yellow), 4.0, Dash::Dashed, true)
    );
    // An arrow takes the stroke, but not the fill or the corners.
    state.tool = Tool::Arrow;
    let edit = state
        .drag(
            &after,
            view,
            &mut camera,
            egui::pos2(100.0, 300.0),
            egui::pos2(200.0, 300.0),
            false,
        )
        .unwrap();
    let arrow = applied(&after, &edit);
    let made = arrow.elements().last().unwrap();
    assert_eq!(
        (made.stroke, made.fill, made.width, made.dash, made.rounded),
        (Color::Green, None, 4.0, Dash::Dashed, false)
    );
    // And text, the color and the size.
    state.tool = Tool::Text;
    state.write(&arrow, Subject::New(Point::new(0.0, 0.0)));
    let edit = wrote(&mut state, &arrow, "hi").unwrap();
    let written = applied(&arrow, &edit);
    let made = written.elements().last().unwrap();
    assert_eq!(made.stroke, Color::Green);
    assert_eq!(made.width, STROKE_WIDTH);
    assert!(matches!(made.shape, Shape::Text { size, .. } if size == 28.0));
}

#[test]
fn a_style_shows_what_an_element_has() {
    let mut boxed = filled(1, Shape::Rectangle(Rect::new(0.0, 0.0, 10.0, 10.0)));
    boxed.rounded = true;
    boxed.dash = Dash::Dotted;
    let style = Style::default().of(&boxed);
    assert_eq!(
        (style.fill, style.rounded, style.dash),
        (Some(Color::Blue), true, Dash::Dotted)
    );
    // Text has no stroke to show: those stay as they were.
    let style = style.of(&text(2, Point::new(0.0, 0.0), "words"));
    assert_eq!(
        (style.fill, style.dash, style.size),
        (Some(Color::Blue), Dash::Dotted, TEXT_SIZE)
    );
}

#[test]
fn styles_are_kept_in_the_file() {
    let mut boxed = rectangle(1);
    boxed.dash = Dash::Dashed;
    boxed.rounded = true;
    let diagram = Diagram::new(vec![boxed, rectangle(2)]).unwrap();
    let written = encoded(&diagram);
    assert!(
        written.contains("h: 50), dash: Dashed, rounded: true),"),
        "{written}"
    );
    assert_eq!(
        written.matches("dash").count(),
        1,
        "a solid stroke isn't written"
    );
    assert_eq!(Diagram::decode(written.as_bytes()).unwrap(), diagram);
}

// ── The pen ──────────────────────────────────────────────────────────────────

#[test]
fn simplifying_keeps_the_points_that_matter() {
    // Along a line, with a wobble under the tolerance, then round a corner.
    let drawn: Vec<Point> = (0..=20)
        .map(|step| Point::new(step as f32 * 5.0, if step % 2 == 0 { 0.0 } else { 0.3 }))
        .chain((1..=10).map(|step| Point::new(100.0, step as f32 * 5.0)))
        .collect();
    assert_eq!(
        edit::simplify(&drawn, 0.5),
        vec![
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(100.0, 50.0)
        ]
    );
    // Looked at more closely, the wobble matters.
    assert!(edit::simplify(&drawn, 0.1).len() > 10);
    // Nothing to leave out of a line or less.
    assert_eq!(edit::simplify(&drawn[..2], 0.5), drawn[..2].to_vec());
    assert_eq!(edit::simplify(&[], 0.5), Vec::<Point>::new());
}

#[test]
fn a_stroke_is_kept_to_a_tenth_and_is_at_least_a_line() {
    let drawn = [
        Point::new(0.04, 0.0),
        Point::new(10.26, 5.123),
        Point::new(20.0, -3.96),
    ];
    let made = edit::stroke(ElementId(4), &drawn, 0.5).unwrap();
    assert_eq!(made.id, ElementId(4));
    assert_eq!(
        made.shape,
        Shape::Freehand {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(10.3, 5.1),
                Point::new(20.0, -4.0)
            ]
        }
    );
    // The pointer held still isn't a stroke.
    let still = [Point::new(5.0, 5.0), Point::new(5.01, 5.02)];
    assert_eq!(edit::stroke(ElementId(1), &still, 0.5), None);
    assert_eq!(edit::stroke(ElementId(1), &still[..1], 0.5), None);
}

#[test]
fn a_smooth_curve_goes_through_every_point() {
    let points = [
        Point::new(0.0, 0.0),
        Point::new(40.0, 30.0),
        Point::new(80.0, 0.0),
        Point::new(120.0, 30.0),
    ];
    let curve = edit::smooth(&points, 4.0);
    assert!(curve.len() > 30, "{}", curve.len());
    for point in &points {
        assert!(curve.contains(point), "{point:?} isn't on the curve");
    }
    assert_eq!((curve[0], *curve.last().unwrap()), (points[0], points[3]));
    // No two of its points are far apart, and it rounds the corners: it
    // leaves the straight lines between the points given.
    for pair in curve.windows(2) {
        assert!(
            (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y) < 6.0,
            "{pair:?}"
        );
    }
    let straight = outline(
        1,
        Shape::Freehand {
            points: points.to_vec(),
        },
    );
    assert!(
        curve
            .iter()
            .any(|point| !edit::hit(&straight, *point, 0.0, &Element::bounds))
    );
    // A line stays a line.
    assert_eq!(edit::smooth(&points[..2], 4.0), points[..2].to_vec());
}

#[test]
fn the_pen_draws_a_stroke_in_the_style_picked_and_stays_in_hand() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.pick(Tool::Pen);
    state.restyle(&diagram, Restyle::Stroke(Color::Magenta));
    state.restyle(&diagram, Restyle::Fill(Some(Color::Blue)));
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(500.0, 300.0),
            egui::pos2(560.0, 340.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Draw");
    let after = applied(&diagram, &edit);
    let made = after.elements().last().unwrap();
    assert_eq!(
        made.shape,
        Shape::Freehand {
            points: vec![Point::new(500.0, 300.0), Point::new(560.0, 340.0)]
        }
    );
    assert_eq!(
        (made.stroke, made.fill),
        (Color::Magenta, None),
        "a stroke has no fill"
    );
    assert_eq!(state.tool, Tool::Pen);
    assert!(state.selection.is_empty());
    // It doesn't bind to what it's drawn over, and a press that goes nowhere
    // draws nothing.
    let at = egui::pos2(150.0, 130.0);
    assert_eq!(state.drag(&after, view, &mut camera, at, at, false), None);
}

// ── Copies, order, groups, aligning and snapping ─────────────────────────────

/// The ids, back to front.
fn order(diagram: &Diagram) -> Vec<u64> {
    diagram
        .elements()
        .iter()
        .map(|element| element.id.0)
        .collect()
}

/// The changes made, and then undone as history would: the diagram after.
fn made_and_undone(diagram: &Diagram, changes: &[Change]) -> Diagram {
    let mut after = diagram.clone();
    let inverses: Vec<Change> = changes
        .iter()
        .map(|change| after.apply(change).unwrap())
        .collect();
    let mut undone = after.clone();
    for inverse in inverses.iter().rev() {
        undone.apply(inverse).unwrap();
    }
    assert_eq!(undone, *diagram, "undoing doesn't give back what was");
    after
}

fn five() -> Diagram {
    Diagram::new((1..=5).map(rectangle).collect()).unwrap()
}

#[test]
fn copies_are_new_elements_bound_to_each_other() {
    let diagram = joined();
    let all: Vec<&Element> = diagram.elements().iter().collect();
    let copies = edit::copies(&all, &diagram, 0.0, 200.0);
    assert_eq!(
        copies.iter().map(|copy| copy.id.0).collect::<Vec<_>>(),
        vec![4, 5, 6]
    );
    let after = applied(
        &diagram,
        &Edit {
            label: String::new(),
            changes: edit::insertions(&diagram, copies),
        },
    );
    // The copied arrow joins the copied boxes, not the originals.
    let (points, from, to) = ends(&after, 6);
    assert_eq!((from, to), (Some(ElementId(4)), Some(ElementId(5))));
    assert_eq!(points[0].y, ends(&diagram, 3).0[0].y + 200.0);

    // Copied without its boxes, it's bound to nothing.
    let arrow = [diagram.get(ElementId(3)).unwrap()];
    let alone = edit::copies(&arrow, &diagram, 5.0, 5.0);
    assert!(matches!(
        alone[0].shape,
        Shape::Line {
            from: None,
            to: None,
            ..
        }
    ));
}

#[test]
fn duplicating_selects_the_copies() {
    let diagram = two_boxes();
    let mut state = State::default();
    state.selection = vec![ElementId(2)];
    let edit = state.duplicate(&diagram).unwrap();
    assert_eq!(edit.label, "Duplicate");
    let after = applied(&diagram, &edit);
    assert_eq!(order(&after), vec![1, 2, 3]);
    assert_eq!(
        after.get(ElementId(3)).unwrap().shape,
        Shape::Rectangle(Rect::new(310.0, 110.0, 100.0, 60.0))
    );
    assert_eq!(state.selection, vec![ElementId(3)]);
    state.selection.clear();
    assert_eq!(state.duplicate(&after), None);
}

#[test]
fn a_move_with_copy_held_leaves_the_original() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.copy = true;
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(150.0, 330.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Duplicate");
    let after = applied(&diagram, &edit);
    assert_eq!(after.get(ElementId(1)), diagram.get(ElementId(1)));
    assert_eq!(
        after.get(ElementId(3)).unwrap().shape,
        Shape::Rectangle(Rect::new(100.0, 300.0, 100.0, 60.0))
    );
    assert_eq!(state.selection, vec![ElementId(3)]);
}

#[test]
fn what_is_copied_is_pasted_where_asked() {
    let diagram = joined();
    let mut state = State::default();
    assert_eq!(state.copied(&diagram), None);
    state.selection = vec![ElementId(1), ElementId(2), ElementId(3)];
    let text = state.copied(&diagram).unwrap();
    // It's a diagram file of them.
    assert_eq!(Diagram::decode(text.as_bytes()).unwrap(), diagram);

    // Into another diagram, whose ids it mustn't take: its middle where asked.
    let other = Diagram::new(vec![rectangle(1), rectangle(2)]).unwrap();
    let mut state = State::default();
    state.tool = Tool::Rectangle;
    let edit = state
        .paste(&other, &text, Point::new(1000.0, 1000.0), &Element::bounds)
        .unwrap();
    assert_eq!(edit.label, "Paste");
    let after = applied(&other, &edit);
    assert_eq!(order(&after), vec![1, 2, 3, 4, 5]);
    assert_eq!(
        state.selection,
        vec![ElementId(3), ElementId(4), ElementId(5)]
    );
    assert_eq!(state.tool, Tool::Select);
    let pasted: Vec<&Element> = after.elements()[2..].iter().collect();
    let around = pasted
        .iter()
        .map(|e| e.bounds())
        .reduce(|a, b| a.union(&b))
        .unwrap();
    assert_eq!(around.center(), Point::new(1000.0, 1000.0));
    assert_eq!(
        ends(&after, 5).1,
        Some(ElementId(3)),
        "still bound, to the pasted box"
    );
}

#[test]
fn other_text_is_pasted_as_text() {
    let diagram = two_boxes();
    let mut state = State::default();
    let edit = state
        .paste(
            &diagram,
            "just words\n",
            Point::new(5.0, 6.0),
            &Element::bounds,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    assert_eq!(
        after.elements().last().unwrap().shape,
        Shape::Text {
            at: Point::new(5.0, 6.0),
            text: "just words".into(),
            size: TEXT_SIZE
        }
    );
    assert_eq!(
        state.paste(&diagram, "  \n", Point::default(), &Element::bounds),
        None
    );
}

#[test]
fn elements_move_forward_and_back_among_the_rest() {
    let diagram = five();
    let moved = |ids: &[u64], to: Order| {
        let ids: Vec<ElementId> = ids.iter().map(|id| ElementId(*id)).collect();
        order(&made_and_undone(
            &diagram,
            &edit::reorder(&diagram, &ids, to),
        ))
    };
    assert_eq!(moved(&[2, 4], Order::Front), vec![1, 3, 5, 2, 4]);
    assert_eq!(moved(&[2, 4], Order::Back), vec![2, 4, 1, 3, 5]);
    assert_eq!(moved(&[2, 4], Order::Forward), vec![1, 3, 2, 5, 4]);
    assert_eq!(moved(&[2, 4], Order::Backward), vec![2, 1, 4, 3, 5]);
    // Side by side, they step together; at the end, they stay.
    assert_eq!(moved(&[2, 3], Order::Forward), vec![1, 4, 2, 3, 5]);
    assert_eq!(moved(&[4, 5], Order::Forward), vec![1, 2, 3, 4, 5]);
    assert!(edit::reorder(&diagram, &[ElementId(5)], Order::Front).is_empty());
    assert!(edit::reorder(&diagram, &[ElementId(1)], Order::Backward).is_empty());
}

#[test]
fn grouped_elements_are_selected_and_moved_together() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.selection = vec![ElementId(1), ElementId(2)];
    let edit = state
        .group(&diagram, true)
        .unwrap()
        .bound(&diagram, &Element::bounds);
    assert_eq!(edit.label, "Group");
    let grouped = made_and_undone(&diagram, &edit.changes);
    assert_eq!(order(&grouped), vec![3, 1, 2]);
    assert!(grouped.get(ElementId(3)).unwrap().is_group());
    assert_eq!(grouped.get(ElementId(1)).unwrap().group, Some(ElementId(3)));
    assert_eq!(grouped.get(ElementId(2)).unwrap().group, Some(ElementId(3)));
    assert_eq!(
        state.group(&grouped, true),
        None,
        "one group is one thing: nothing to group it with"
    );

    // Dragging one moves both, and both are selected.
    state.selection.clear();
    let edit = state
        .drag(
            &grouped,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(150.0, 230.0),
            false,
        )
        .unwrap();
    assert_eq!(
        state.selection,
        vec![ElementId(1), ElementId(3), ElementId(2)]
    );
    assert_eq!(
        applied(&grouped, &edit).bounds(),
        Some(Rect::new(100.0, 200.0, 300.0, 60.0).expand(GROUP_PADDING))
    );

    // In the file, its own under it, and out of the group again.
    let written = encoded(&grouped);
    assert!(
        written.contains(r#"(id: "group-1", shape: Group, children: ["#),
        "{written}"
    );
    assert_eq!(
        written.matches("            (id: \"rect").count(),
        2,
        "{written}"
    );
    assert_eq!(
        encoded(&Diagram::decode(written.as_bytes()).unwrap()),
        written
    );
    let edit = state.group(&grouped, false).unwrap();
    assert_eq!(edit.label, "Ungroup");
    assert_eq!(applied(&grouped, &edit), diagram);
    assert_eq!(state.group(&diagram, false), None);
    // One thing alone isn't a group.
    state.selection = vec![ElementId(1)];
    assert_eq!(state.group(&diagram, true), None);
}

/// Two boxes in a group (9), and a third box in none.
fn a_group_and_another() -> Diagram {
    grouped()
}

#[test]
fn a_double_click_goes_into_a_group_to_move_one_thing() {
    let (view, mut camera) = plain();
    let diagram = a_group_and_another();
    let mut state = State::default();
    let (first, second, third) = (
        Point::new(150.0, 130.0),
        Point::new(350.0, 130.0),
        Point::new(150.0, 330.0),
    );
    let click = |state: &mut State, at, shift, twice| {
        state.click(&diagram, at, 6.0, shift, twice, &Element::bounds);
    };

    // Once selects the group; twice, the one thing, without writing in it.
    click(&mut state, first, false, false);
    assert_eq!(
        state.selection,
        vec![ElementId(1), ElementId(9), ElementId(2)]
    );
    click(&mut state, first, false, true);
    assert_eq!(
        (state.selection.clone(), state.within),
        (vec![ElementId(1)], Some(ElementId(9)))
    );
    assert!(!state.is_writing());

    // Inside, a click picks the other one alone, and a drag moves it alone.
    click(&mut state, second, false, false);
    assert_eq!(state.selection, vec![ElementId(2)]);
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(150.0, 180.0),
            false,
        )
        .unwrap();
    assert_eq!(state.selection, vec![ElementId(1)]);
    let moved = applied(&diagram, &edit);
    assert_eq!(
        moved.get(ElementId(1)).unwrap().bounds(),
        Rect::new(100.0, 150.0, 100.0, 60.0)
    );
    assert_eq!(moved.get(ElementId(2)), diagram.get(ElementId(2)));
    assert_eq!(
        moved.get(ElementId(1)).unwrap().group,
        Some(ElementId(9)),
        "still in the group"
    );

    // Twice again writes in it, as in anything else.
    click(&mut state, first, false, true);
    assert!(state.is_writing());
    state.writing = None;

    // Ungrouping what's picked inside takes that one out, to just in front
    // of the rest of it.
    let edit = state.group(&diagram, false).unwrap();
    let out = applied(&diagram, &edit);
    assert_eq!(out.get(ElementId(1)).unwrap().group, None);
    assert_eq!(order(&out), vec![9, 2, 1, 3]);
    assert_eq!(state.within, None);

    // A click outside the group leaves it: the group is one thing again.
    state.within = Some(ElementId(9));
    state.selection = vec![ElementId(1)];
    click(&mut state, third, false, false);
    assert_eq!(
        (state.selection.clone(), state.within),
        (vec![ElementId(3)], None)
    );
    click(&mut state, second, false, false);
    assert_eq!(
        state.selection,
        vec![ElementId(2), ElementId(9), ElementId(1)]
    );
}

#[test]
fn shift_adds_a_whole_group_and_takes_it_away() {
    let diagram = a_group_and_another();
    let mut state = State::default();
    let (first, second, third) = (
        Point::new(150.0, 130.0),
        Point::new(350.0, 130.0),
        Point::new(150.0, 330.0),
    );
    state.click(&diagram, third, 6.0, false, false, &Element::bounds);
    state.click(&diagram, first, 6.0, true, false, &Element::bounds);
    assert_eq!(
        state.selection,
        vec![ElementId(3), ElementId(1), ElementId(9), ElementId(2)]
    );
    state.click(&diagram, second, 6.0, true, false, &Element::bounds);
    assert_eq!(state.selection, vec![ElementId(3)]);
    // On nothing: with Shift the selection stays, without it's let go of.
    state.click(
        &diagram,
        Point::new(600.0, 500.0),
        6.0,
        true,
        false,
        &Element::bounds,
    );
    assert_eq!(state.selection, vec![ElementId(3)]);
    state.click(
        &diagram,
        Point::new(600.0, 500.0),
        6.0,
        false,
        false,
        &Element::bounds,
    );
    assert!(state.selection.is_empty());
}

#[test]
fn the_menu_is_for_what_it_is_asked_on() {
    let diagram = a_group_and_another();
    let mut state = State::default();
    state.tool = Tool::Rectangle;
    let (first, third, nothing) = (
        Point::new(150.0, 130.0),
        Point::new(150.0, 330.0),
        Point::new(600.0, 500.0),
    );

    // On something not selected: that (with its group), with the Select tool.
    state.point_at(&diagram, first, 6.0, &Element::bounds);
    assert_eq!(
        (state.selection.clone(), state.tool),
        (vec![ElementId(1), ElementId(9), ElementId(2)], Tool::Select)
    );
    // On something selected, the selection stays as it is.
    state.selection = vec![ElementId(1), ElementId(9), ElementId(2), ElementId(3)];
    state.point_at(&diagram, third, 6.0, &Element::bounds);
    assert_eq!(state.selection.len(), 4);
    // On nothing: the canvas's menu, whose Paste is asked of the panel, there.
    state.point_at(&diagram, nothing, 6.0, &Element::bounds);
    assert!(state.selection.is_empty());
    let ctx = egui::Context::default();
    assert_eq!(
        state.pick_of(&ctx, &diagram, Pick::Paste, &Element::bounds),
        None
    );
    assert_eq!(state.asked, Some(Request::Paste(nothing)));

    // What it picks is done to the selection, as the keys do it.
    state.selection = vec![ElementId(1), ElementId(2)];
    let edit = state
        .pick_of(&ctx, &diagram, Pick::Group(false), &Element::bounds)
        .unwrap();
    assert_eq!(edit.label, "Ungroup");
    assert!(
        applied(&diagram, &edit)
            .elements()
            .iter()
            .all(|element| element.group.is_none())
    );
    let edit = state
        .pick_of(&ctx, &diagram, Pick::Delete, &Element::bounds)
        .unwrap()
        .bound(&diagram, &Element::bounds);
    // (The group, with nothing left in it, goes too.)
    assert_eq!(applied(&diagram, &edit).elements().len(), 1);
    assert!(state.selection.is_empty());
}

#[test]
fn a_line_bends_where_its_middle_is_dragged_and_straightens_again() {
    let (view, mut camera) = plain();
    let diagram = Diagram::new(vec![line(1, (100.0, 100.0), (300.0, 100.0))]).unwrap();
    let mut state = State::default();
    state.selection = vec![ElementId(1)];
    let points = |diagram: &Diagram| ends(diagram, 1).0;

    // The middle of the segment, dragged, is a new point; the ends stay.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(200.0, 100.0),
            egui::pos2(200.0, 160.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Add Point");
    let bent = applied(&diagram, &edit);
    assert_eq!(
        points(&bent),
        vec![
            Point::new(100.0, 100.0),
            Point::new(200.0, 160.0),
            Point::new(300.0, 100.0)
        ]
    );
    // Away from the middle, the line is moved instead.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 100.0),
            egui::pos2(150.0, 160.0),
            false,
        )
        .unwrap();
    assert_eq!(edit.label, "Move");

    // The menu adds one where it's asked for, in the segment nearest.
    state.point_at(&bent, Point::new(250.0, 131.0), 6.0, &Element::bounds);
    let ctx = egui::Context::default();
    let edit = state
        .pick_of(&ctx, &bent, Pick::AddPoint, &Element::bounds)
        .unwrap();
    assert_eq!(points(&applied(&bent, &edit))[2], Point::new(250.0, 131.0));

    // A point it bends at is taken out; its ends aren't.
    let edit = state.remove_point(&bent, 1).unwrap();
    assert_eq!(edit.label, "Remove Point");
    assert_eq!(applied(&bent, &edit), diagram);
    assert_eq!(state.remove_point(&bent, 0), None);
    assert_eq!(state.remove_point(&bent, 2), None);
}

#[test]
fn a_line_s_heads_are_picked_and_it_turns_around() {
    let diagram = joined();
    let arrow = diagram.elements().last().unwrap().id;
    let mut state = State::default();
    state.selection = vec![arrow, ElementId(1)];
    let heads = |diagram: &Diagram| edit::heads(diagram.get(arrow).unwrap()).unwrap();
    assert_eq!(heads(&diagram), (Head::None, Head::Arrow));

    // Only the line takes a head.
    let edit = state
        .restyle(&diagram, Restyle::Start(Head::Arrow))
        .unwrap();
    assert_eq!(edit.changes.len(), 1);
    let both = applied(&diagram, &edit);
    assert_eq!(heads(&both), (Head::Arrow, Head::Arrow));
    let edit = state.restyle(&both, Restyle::End(Head::None)).unwrap();
    assert_eq!(heads(&applied(&both, &edit)), (Head::Arrow, Head::None));

    // Turned around, it's bound the other way, between the same places.
    let edit = state
        .reverse(&diagram)
        .unwrap()
        .bound(&diagram, &Element::bounds);
    let turned = applied(&diagram, &edit);
    assert_eq!(heads(&turned), (Head::Arrow, Head::None));
    let ((before, from, to), (after, back_from, back_to)) =
        (ends(&diagram, arrow.0), ends(&turned, arrow.0));
    assert_eq!((back_from, back_to), (to, from));
    assert_eq!(after, before.into_iter().rev().collect::<Vec<_>>());
}

#[test]
fn copies_of_a_group_are_a_group_of_their_own() {
    let diagram = two_boxes();
    let ids = [ElementId(1), ElementId(2)];
    let grouped = applied(
        &diagram,
        &Edit {
            label: String::new(),
            changes: edit::group(&diagram, &ids),
        },
    );
    let all: Vec<&Element> = grouped.elements().iter().collect();
    let copies = edit::copies(&all, &grouped, 0.0, 100.0);
    assert_eq!(
        copies.iter().map(|copy| copy.group).collect::<Vec<_>>(),
        vec![None, Some(ElementId(4)), Some(ElementId(4))]
    );
}

#[test]
fn aligning_lines_things_up_and_a_group_moves_as_one() {
    let diagram = Diagram::new(vec![
        outline(1, Shape::Rectangle(Rect::new(0.0, 0.0, 100.0, 40.0))),
        outline(2, Shape::Rectangle(Rect::new(50.0, 100.0, 20.0, 20.0))),
        outline(3, Shape::Ellipse(Rect::new(300.0, 200.0, 60.0, 80.0))),
    ])
    .unwrap();
    let all = [ElementId(1), ElementId(2), ElementId(3)];
    let lefts = |diagram: &Diagram| -> Vec<f32> {
        diagram
            .elements()
            .iter()
            .filter(|e| !e.is_group())
            .map(|e| e.bounds().x)
            .collect()
    };
    let aligned = |to: Align| {
        made_and_undone(
            &diagram,
            &edit::align(&diagram, &all, to, None, &Element::bounds),
        )
    };
    assert_eq!(lefts(&aligned(Align::Left)), vec![0.0, 0.0, 0.0]);
    assert_eq!(lefts(&aligned(Align::Right)), vec![260.0, 340.0, 300.0]);
    assert_eq!(lefts(&aligned(Align::Center)), vec![130.0, 170.0, 150.0]);
    let tops: Vec<f32> = aligned(Align::Bottom)
        .elements()
        .iter()
        .map(|e| e.bounds().y)
        .collect();
    assert_eq!(tops, vec![240.0, 260.0, 200.0]);
    let middles: Vec<f32> = aligned(Align::Middle)
        .elements()
        .iter()
        .map(|e| e.bounds().center().y)
        .collect();
    assert_eq!(middles, vec![140.0, 140.0, 140.0]);
    assert_eq!(aligned(Align::Top).bounds().unwrap().h, 80.0);

    // The first two as a group keep their places by each other.
    let grouped = applied(
        &diagram,
        &Edit {
            label: String::new(),
            changes: edit::group(&diagram, &all[..2]),
        },
    );
    let right = applied(
        &grouped,
        &Edit {
            label: String::new(),
            changes: edit::align(&grouped, &all, Align::Right, None, &Element::bounds),
        },
    );
    assert_eq!(lefts(&right), vec![260.0, 310.0, 300.0]);
    // One thing (a group is one) has nothing to line up with.
    assert!(edit::align(&grouped, &all[..2], Align::Left, None, &Element::bounds).is_empty());
    assert_eq!(edit::unit_count(&grouped, &all, None), 2);
    // Gone into, its elements line up with each other.
    let group = Some(ElementId(4));
    assert_eq!(edit::unit_count(&grouped, &all[..2], group), 2);
    let inside = made_and_undone(
        &grouped,
        &edit::align(&grouped, &all[..2], Align::Left, group, &Element::bounds),
    );
    assert_eq!(lefts(&inside), vec![0.0, 0.0, 300.0]);
}

#[test]
fn a_moved_box_snaps_into_line_with_another() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.snap = Some(6.0);
    // The first box (100..200 by 100..160), dragged to 4 short of under the
    // second (300..400): it lines up with it.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(346.0, 332.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    assert_eq!(
        after.get(ElementId(1)).unwrap().shape,
        Shape::Rectangle(Rect::new(300.0, 302.0, 100.0, 60.0)),
        "across, not down: nothing's near that way"
    );
    // Further off, or with snapping held off, it goes where it's dragged.
    state.selection.clear();
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(340.0, 332.0),
            false,
        )
        .unwrap();
    assert_eq!(
        applied(&diagram, &edit)
            .get(ElementId(1))
            .unwrap()
            .bounds()
            .x,
        290.0
    );
    state.snap = None;
    state.selection.clear();
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 130.0),
            egui::pos2(346.0, 332.0),
            false,
        )
        .unwrap();
    assert_eq!(
        applied(&diagram, &edit)
            .get(ElementId(1))
            .unwrap()
            .bounds()
            .x,
        296.0
    );
}

#[test]
fn snapping_finds_the_nearest_side_or_middle_each_way() {
    let others = [
        Rect::new(0.0, 0.0, 100.0, 60.0),
        Rect::new(300.0, 200.0, 50.0, 50.0),
    ];
    // Its middle near the first's middle; its bottom near the second's top.
    let snap = edit::snap_rect(Rect::new(32.0, 147.0, 40.0, 50.0), &others, 5.0);
    assert_eq!((snap.dx, snap.x), (-2.0, Some(50.0)));
    assert_eq!((snap.dy, snap.y), (3.0, Some(200.0)));
    let none = edit::snap_rect(Rect::new(140.0, 90.0, 20.0, 20.0), &others, 5.0);
    assert_eq!(none, edit::Snap::default());
    // A point, to a side.
    let snap = edit::snap_point(Point::new(103.0, 500.0), &others, 5.0);
    assert_eq!((snap.dx, snap.dy), (-3.0, 0.0));
    // Lines and strokes aren't snapped to, nor what's being dragged.
    let diagram = Diagram::new(vec![
        rectangle(1),
        line(2, (0.0, 0.0), (9.0, 9.0)),
        rectangle(3),
    ])
    .unwrap();
    assert_eq!(
        edit::snap_targets(&diagram, &[ElementId(3)], &Element::bounds),
        vec![rectangle(1).bounds()]
    );
}

#[test]
fn a_box_being_made_or_resized_snaps_its_corner() {
    let (view, mut camera) = plain();
    let diagram = two_boxes();
    let mut state = State::default();
    state.snap = Some(6.0);
    state.tool = Tool::Rectangle;
    // Its right side to 3 past the first box's right, its bottom far from anything.
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(120.0, 300.0),
            egui::pos2(203.0, 400.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    assert_eq!(
        after.get(ElementId(3)).unwrap().shape,
        Shape::Rectangle(Rect::new(120.0, 300.0, 80.0, 100.0))
    );
    // The first box's right side handle, dragged to near the second's left:
    // it snaps across, and stays as it is up and down.
    state.selection = vec![ElementId(1)];
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(205.0, 130.0),
            egui::pos2(302.0, 158.0),
            false,
        )
        .unwrap();
    assert_eq!(
        applied(&diagram, &edit).get(ElementId(1)).unwrap().shape,
        Shape::Rectangle(Rect::new(100.0, 100.0, 200.0, 60.0))
    );
}

// ── Pictures ─────────────────────────────────────────────────────────────────

fn inks() -> crate::export::Inks {
    crate::export::Inks {
        background: [0x10, 0x20, 0x30],
        named: [
            [0xee, 0xee, 0xee],
            [0x88, 0x88, 0x88],
            [0x00, 0xaa, 0x00],
            [0xff, 0x00, 0x00],
            [0x00, 0xff, 0x00],
            [0xff, 0xff, 0x00],
            [0x00, 0x00, 0xff],
            [0xff, 0x00, 0xff],
            [0x00, 0xff, 0xff],
        ],
    }
}

fn fixture() -> Diagram {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/diagram/sample.diagram"
    );
    Diagram::decode(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn the_fixture_is_as_the_viewer_writes_it() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/diagram/sample.diagram"
    );
    assert_eq!(encoded(&fixture()), std::fs::read_to_string(path).unwrap());
}

#[test]
fn an_svg_draws_each_element_in_the_theme_s_colors() {
    let mut boxed = filled(1, Shape::Rectangle(Rect::new(0.0, 0.0, 100.0, 60.0)));
    boxed.label = "A < B\n& \"C\"".into();
    boxed.rounded = true;
    boxed.dash = Dash::Dashed;
    let mut arrow = line(2, (100.0, 30.0), (200.0, 30.0));
    arrow.stroke = Color::Red;
    arrow.label = "go".into();
    let elements = [
        boxed,
        arrow,
        outline(3, Shape::Ellipse(Rect::new(200.0, 0.0, 100.0, 60.0))),
        outline(4, Shape::Diamond(Rect::new(0.0, 100.0, 100.0, 60.0))),
        text(5, Point::new(200.0, 100.0), "two\nlines"),
        outline(
            6,
            Shape::Freehand {
                points: vec![
                    Point::new(0.0, 200.0),
                    Point::new(50.0, 220.0),
                    Point::new(100.0, 200.0),
                ],
            },
        ),
    ];
    let all: Vec<&Element> = elements.iter().collect();
    let svg = crate::export::svg(&all, &inks(), 2.0).unwrap();
    // The box around everything, with room, at twice the size; on the background.
    assert!(svg.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" width="696" height="536" viewBox="-24 -24 348 268""#), "{svg}");
    assert!(
        svg.contains(r##"<rect x="-24" y="-24" width="348" height="268" fill="#102030"/>"##),
        "{svg}"
    );
    for part in [
        // The rounded, dashed, tinted box, and its label, escaped, a line each.
        r##"<rect x="0" y="0" width="100" height="60" rx="15" fill="#0000ff" fill-opacity="0.22" stroke="#eeeeee" stroke-width="2" stroke-dasharray="10 7"/>"##,
        r#"text-anchor="middle""#,
        "A &lt; B</tspan>",
        "&amp; &quot;C&quot;</tspan>",
        // The arrow, its head, and its label over it.
        r##"<polyline points="100,30 200,30" fill="none" stroke="#ff0000" stroke-width="2"/>"##,
        ">go</tspan>",
        r#"<ellipse cx="250" cy="30" rx="50" ry="30" fill="none""#,
        r#"<polygon points="50,100 100,130 50,160 0,130" fill="none""#,
        ">two</tspan>",
        ">lines</tspan>",
        r#"stroke-linejoin="round""#,
    ] {
        assert!(svg.contains(part), "no {part} in:\n{svg}");
    }
    assert_eq!(
        svg.matches("<polyline").count(),
        3,
        "the line, its head, the stroke"
    );
    assert_eq!(crate::export::svg(&[], &inks(), 1.0), None);
}

#[test]
fn a_png_is_the_svg_rendered() {
    let theme = bedtime_ui::theme::Theme::dark();
    let diagram = fixture();
    let svg = crate::svg(&diagram, &[], &theme, crate::PNG_SCALE).unwrap();
    let (size, rgba) = crate::rendered(&svg).unwrap();
    // 720 by 270 of diagram and the room around it (768 by 318), made
    // large: 2048 across.
    assert_eq!(size, [2048, 848]);
    assert_eq!(rgba.len(), 2048 * 848 * 4);
    // The corner is the theme's panel, and something else is drawn in it.
    let corner = [rgba[0], rgba[1], rgba[2], rgba[3]];
    let panel = bedtime_ui::theme::color(theme.palette.panel_background);
    assert_eq!(corner, [panel.r(), panel.g(), panel.b(), 255]);
    assert!(rgba.chunks(4).any(|pixel| pixel != corner));
    let png = crate::png(&svg).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

    // Only what's asked for: one box is a smaller picture.
    let one = crate::svg(&diagram, &[ElementId(1)], &theme, 1.0).unwrap();
    let [width, height] = crate::rendered(&one).unwrap().0;
    assert_eq!(width, 2048);
    assert!(
        (height as f32 / width as f32 - 128.0 / 228.0).abs() < 0.01,
        "{height}"
    );
    assert_eq!(crate::svg(&Diagram::default(), &[], &theme, 1.0), None);
}

// ── Names, the file's shorthands, and groups as elements ─────────────────────

#[test]
fn elements_are_named_by_kind_unless_named() {
    let diagram = sample();
    let names: Vec<&str> = diagram
        .elements()
        .iter()
        .map(|element| element.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "rect-1",
            "arrow-1",
            "text-1",
            "freehand-1",
            "ellipse-1",
            "diamond-1"
        ]
    );
    // One added with no name is named above any of its kind; a name taken
    // is refused.
    let mut after = diagram.clone();
    let mut named = rectangle(8);
    named.name = "rect-7".into();
    after
        .apply(&Change::Insert {
            index: 0,
            element: named,
        })
        .unwrap();
    after
        .apply(&Change::Insert {
            index: 0,
            element: rectangle(9),
        })
        .unwrap();
    assert_eq!(after.get(ElementId(9)).unwrap().name, "rect-8");
    let mut twice = rectangle(10);
    twice.name = "rect-1".into();
    assert!(
        after
            .apply(&Change::Insert {
                index: 0,
                element: twice
            })
            .is_err()
    );
    assert!(is_made_name("arrow-12") && !is_made_name("api-2"));
}

#[test]
fn files_name_what_lines_are_bound_to_and_leave_out_where_they_meet() {
    let diagram = joined();
    let written = encoded(&diagram);
    assert!(
        written.contains(r#"(id: "arrow-1", shape: Arrow(from: "rect-1", to: "rect-2")),"#),
        "{written}"
    );
    assert_eq!(Diagram::decode(written.as_bytes()).unwrap(), diagram);
}

#[test]
fn files_written_by_people_and_models_are_read() {
    let diagram = Diagram::decode(
        br#"(
            version: 1,
            elements: [
                (id: "api", shape: Rectangle(x: 0, y: 0, w: 100, h: 50), fill: Some(Blue)),
                (id: "backend", shape: Group, label: "Backend", padding: 10, children: [
                    (id: "db", shape: Ellipse(x: 0, y: 200, w: 100, h: 50)),
                    (shape: Text(at: (0, 260), text: "Postgres", size: 16)),
                ]),
                (shape: Arrow(from: "api", to: "db", via: [(50.5, 120)]), label: "SQL"),
                (shape: Line(from: (0, -40), to: "backend", start: Arrow)),
            ],
        )"#,
    )
    .unwrap();
    let names: Vec<&str> = diagram
        .elements()
        .iter()
        .map(|element| element.name.as_str())
        .collect();
    // (A line with a head at either end is an arrow.)
    assert_eq!(
        names,
        ["api", "backend", "db", "text-1", "arrow-1", "arrow-2"]
    );
    let group = diagram.named("backend").unwrap().id;
    assert_eq!(diagram.named("db").unwrap().group, Some(group));
    assert_eq!(diagram.named("text-1").unwrap().group, Some(group));
    assert_eq!(diagram.named("api").unwrap().fill, Some(Color::Blue));
    // The group's box is around its own, its padding out, with room above
    // for its label.
    let around = diagram.named("backend").unwrap().bounds();
    assert_eq!(around.x, -10.0);
    assert_eq!(around.y, 200.0 - 10.0 - GROUP_LABEL_ROOM);
    // The arrow's ends are on what it's bound to, through its bend.
    let (points, from, to) = ends(&diagram, 5);
    assert_eq!((from, to), (Some(ElementId(1)), Some(ElementId(3))));
    assert_eq!(points.len(), 3);
    assert_eq!(points[1], Point::new(50.5, 120.0));
    assert!(points[0].y > 50.0 && points[2].y < 200.0, "{points:?}");
    // The line goes from a point to the group's box.
    let (points, from, to) = ends(&diagram, 6);
    assert_eq!((from, to), (None, Some(group)));
    assert_eq!(points[0], Point::new(0.0, -40.0));
    assert!(
        (points[1].y - (around.y - edit::BIND_GAP - 1.0)).abs() < 1.0,
        "{points:?}"
    );
    let Shape::Line { start, end, .. } = &diagram.elements()[5].shape else {
        panic!()
    };
    assert_eq!((*start, *end), (Head::Arrow, Head::None));

    // Written back as it would be written: names for what's bound, whole
    // numbers as whole, `Some` left out, the group's own indented under it.
    let written = encoded(&diagram);
    for part in [
        r#"(id: "api", shape: Rectangle(x: 0, y: 0, w: 100, h: 50), fill: Blue),"#,
        r#"        (id: "backend", shape: Group, label: "Backend", padding: 10, children: ["#,
        r#"            (id: "db", shape: Ellipse(x: 0, y: 200, w: 100, h: 50)),"#,
        "        ]),",
        r#"(id: "arrow-1", shape: Arrow(from: "api", to: "db", via: [(50.5, 120)]), label: "SQL"),"#,
        r#"(id: "arrow-2", shape: Line(from: (0, -40), to: "backend", start: Arrow)),"#,
    ] {
        assert!(written.contains(part), "no {part} in:\n{written}");
    }
    assert_eq!(Diagram::decode(written.as_bytes()).unwrap(), diagram);
}

#[test]
fn numbers_are_written_to_a_hundredth() {
    let diagram = Diagram::new(vec![outline(
        1,
        Shape::Rectangle(Rect::new(37.037086, -601.5562, 0.5, 168.0)),
    )])
    .unwrap();
    assert!(
        encoded(&diagram).contains("Rectangle(x: 37.04, y: -601.56, w: 0.5, h: 168)"),
        "{}",
        encoded(&diagram)
    );
}

#[test]
fn files_that_cannot_be_joined_up_are_refused() {
    let refused = |text: &str| Diagram::decode(text.as_bytes()).unwrap_err();
    let error = refused(r#"(version: 1, elements: [(shape: Arrow(from: (0, 0), to: "nowhere"))])"#);
    assert!(error.contains("\"nowhere\""), "{error}");
    let error = refused(
        r#"(version: 1, elements: [(id: "a", shape: Rectangle(x: 0, y: 0, w: 1, h: 1), children: [
            (shape: Rectangle(x: 0, y: 0, w: 1, h: 1)),
        ])])"#,
    );
    assert!(error.contains("isn't a group"), "{error}");
    let error = refused(
        r#"(version: 1, elements: [(id: "a", shape: Group, children: [
            (id: "b", shape: Group, children: [(shape: Rectangle(x: 0, y: 0, w: 1, h: 1))]),
        ])])"#,
    );
    assert!(error.contains("group in a group"), "{error}");
    let error =
        refused(r#"(version: 1, elements: [(id: "a", shape: Arrow(from: "a", to: (1, 1)))])"#);
    assert!(error.contains("itself"), "{error}");
}

/// Two boxes in a group (9), and a box beside it, on its own.
fn grouped() -> Diagram {
    let group = outline(
        9,
        Shape::Group {
            padding: GROUP_PADDING,
            around: Rect::default(),
        },
    );
    let mut first = filled(1, Shape::Rectangle(Rect::new(100.0, 100.0, 100.0, 60.0)));
    let mut second = filled(2, Shape::Rectangle(Rect::new(300.0, 100.0, 100.0, 60.0)));
    first.group = Some(ElementId(9));
    second.group = Some(ElementId(9));
    let third = filled(3, Shape::Rectangle(Rect::new(100.0, 300.0, 100.0, 60.0)));
    edit::settled(
        &Diagram::new(vec![group, first, second, third]).unwrap(),
        &Element::bounds,
    )
}

#[test]
fn a_group_s_box_follows_what_is_in_it() {
    let diagram = grouped();
    let padded = Rect::new(100.0, 100.0, 300.0, 60.0).expand(GROUP_PADDING);
    assert_eq!(diagram.get(ElementId(9)).unwrap().bounds(), padded);
    // Moving one of it moves its box, in the same edit.
    let moved = edit::translate(diagram.get(ElementId(2)).unwrap(), 0.0, 100.0);
    let changes = edit::with_bindings(
        &diagram,
        vec![Change::Replace { element: moved }],
        &Element::bounds,
    );
    let after = made_and_undone(&diagram, &changes);
    assert_eq!(
        after.get(ElementId(9)).unwrap().bounds(),
        Rect::new(100.0, 100.0, 300.0, 160.0).expand(GROUP_PADDING)
    );
    // With nothing left in it, it's gone.
    let changes = edit::with_bindings(
        &diagram,
        vec![
            Change::Remove { id: ElementId(1) },
            Change::Remove { id: ElementId(2) },
        ],
        &Element::bounds,
    );
    assert!(
        made_and_undone(&diagram, &changes)
            .get(ElementId(9))
            .is_none()
    );
}

#[test]
fn a_line_binds_to_a_group_between_or_around_what_is_in_it() {
    let diagram = grouped();
    let at = |x, y| edit::bindable_at(&diagram, Point::new(x, y), &Element::bounds, None);
    // On one of it: that one. Between them, or in the padding: the group.
    assert_eq!(at(150.0, 130.0), Some(ElementId(1)));
    assert_eq!(at(250.0, 130.0), Some(ElementId(9)));
    assert_eq!(at(150.0, 86.0), Some(ElementId(9)));
    assert_eq!(at(150.0, 70.0), None);

    // An arrow drawn to it meets its box.
    let (view, mut camera) = plain();
    let mut state = State::default();
    state.tool = Tool::Arrow;
    let edit = state
        .drag(
            &diagram,
            view,
            &mut camera,
            egui::pos2(150.0, 330.0),
            egui::pos2(250.0, 130.0),
            false,
        )
        .unwrap();
    let after = applied(&diagram, &edit);
    let (points, from, to) = ends(&after, 10);
    assert_eq!((from, to), (Some(ElementId(3)), Some(ElementId(9))));
    let around = after.get(ElementId(9)).unwrap().bounds();
    // (A little off it, on the way from its middle.)
    assert!(
        points[1].y > around.bottom() && points[1].y < around.bottom() + 6.0,
        "{points:?} {around:?}"
    );

    // A line in the group doesn't bind to it.
    let mut inside = line(11, (0.0, 0.0), (1.0, 1.0));
    inside.group = Some(ElementId(9));
    let mut all = diagram.elements().to_vec();
    all.push(inside);
    let with_line = Diagram::new(all).unwrap();
    assert_eq!(
        edit::bindable_at(
            &with_line,
            Point::new(250.0, 130.0),
            &Element::bounds,
            Some(ElementId(11))
        ),
        None
    );
}

#[test]
fn a_labelled_group_is_picked_by_its_frame_and_its_label() {
    let mut diagram = grouped();
    let mut group = diagram.get(ElementId(9)).unwrap().clone();
    let reach = 6.0;
    let hit = |group: &Element, x, y| edit::hit(group, Point::new(x, y), reach, &Element::bounds);
    // Without a label it isn't drawn, and isn't picked.
    assert!(!hit(&group, 84.0, 130.0));
    group.label = "Backend".into();
    diagram.apply(&Change::Replace { element: group }).unwrap();
    let diagram = edit::settled(&diagram, &Element::bounds);
    let group = diagram.get(ElementId(9)).unwrap();
    let around = group.bounds();
    assert_eq!(around.y, 100.0 - GROUP_PADDING - GROUP_LABEL_ROOM);
    assert!(hit(group, around.x, 130.0), "its frame");
    assert!(hit(group, 250.0, around.y + 10.0), "its label");
    assert!(!hit(group, 250.0, 130.0), "not inside");
    // A double click there writes its label.
    let mut state = State::default();
    state.click(
        &diagram,
        Point::new(250.0, around.y + 10.0),
        reach,
        false,
        true,
        &Element::bounds,
    );
    assert!(state.is_writing());
    assert_eq!(state.selection, vec![ElementId(9)]);
}

#[test]
fn grouping_makes_a_group_element_before_its_own() {
    let diagram =
        Diagram::new(vec![rectangle(1), rectangle(2), rectangle(3), rectangle(4)]).unwrap();
    let changes = edit::group(&diagram, &[ElementId(4), ElementId(2)]);
    let after = made_and_undone(&diagram, &changes);
    assert_eq!(order(&after), vec![1, 5, 2, 4, 3]);
    let group = after.get(ElementId(5)).unwrap();
    assert!(group.is_group());
    assert_eq!(group.name, "group-1");
    assert_eq!(after.get(ElementId(4)).unwrap().group, Some(ElementId(5)));
    assert_eq!(after.get(ElementId(4)).unwrap().name, "rect-4", "kept");

    // Grouped again with another, the group is undone into the new one.
    let changes = edit::group(&after, &[ElementId(5), ElementId(3)]);
    let again = made_and_undone(&after, &changes);
    assert!(again.get(ElementId(5)).is_none());
    assert_eq!(order(&again), vec![1, 6, 2, 4, 3]);
    assert!(
        again.elements()[2..]
            .iter()
            .all(|element| element.group == Some(ElementId(6)))
    );

    // Taken out alone, one goes in front of the rest of it; a group undone
    // leaves its own where they are.
    let changes = edit::ungroup(&again, &[ElementId(2)]);
    let out = made_and_undone(&again, &changes);
    assert_eq!(order(&out), vec![1, 6, 4, 3, 2]);
    assert_eq!(out.get(ElementId(2)).unwrap().group, None);
    let changes = edit::ungroup(&again, &[ElementId(6)]);
    let undone = made_and_undone(&again, &changes);
    assert_eq!(order(&undone), vec![1, 2, 4, 3]);
    assert!(
        undone
            .elements()
            .iter()
            .all(|element| element.group.is_none())
    );
}

#[test]
fn a_group_moves_among_the_rest_as_one() {
    let diagram = grouped();
    let moved = |diagram: &Diagram, ids: &[u64], to: Order| {
        let ids: Vec<ElementId> = ids.iter().map(|id| ElementId(*id)).collect();
        order(&made_and_undone(diagram, &edit::reorder(diagram, &ids, to)))
    };
    assert_eq!(moved(&diagram, &[3], Order::Backward), vec![3, 9, 1, 2]);
    assert_eq!(
        moved(&diagram, &[9, 1, 2], Order::Forward),
        vec![3, 9, 1, 2]
    );
    // One of it, gone into, moves only among the rest of it.
    assert_eq!(moved(&diagram, &[1], Order::Front), vec![9, 2, 1, 3]);
    assert!(edit::reorder(&diagram, &[ElementId(2)], Order::Forward).is_empty());
}

#[test]
fn copies_of_written_names_take_a_number() {
    let mut named = rectangle(1);
    named.name = "api".into();
    let diagram = Diagram::new(vec![named, rectangle(2)]).unwrap();
    let all: Vec<&Element> = diagram.elements().iter().collect();
    let copies = edit::copies(&all, &diagram, 0.0, 0.0);
    assert_eq!(copies[0].name, "api-2");
    assert_eq!(copies[1].name, "", "made again when it's added");
    let after = made_and_undone(&diagram, &edit::insertions(&diagram, copies));
    assert_eq!(after.get(ElementId(4)).unwrap().name, "rect-2");
}

#[test]
fn a_group_is_as_wide_as_its_label() {
    let mut diagram = grouped();
    let mut group = diagram.get(ElementId(9)).unwrap().clone();
    group.label = "a label much wider than the two boxes in this group are".into();
    diagram.apply(&Change::Replace { element: group }).unwrap();
    let diagram = edit::settled(&diagram, &Element::bounds);
    let around = diagram.get(ElementId(9)).unwrap().bounds();
    let label = 55.0 * GROUP_LABEL_SIZE * 0.6;
    assert!(around.w >= label + 16.0, "{around:?}");
    // It grows to the right: its left side stays with what's in it.
    assert_eq!(around.x, 100.0 - GROUP_PADDING);
}
