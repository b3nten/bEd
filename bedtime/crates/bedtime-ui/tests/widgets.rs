//! Components, drawn headlessly: a bare egui context has the dark theme and
//! egui's own style, which is enough to check sizes, names and layout.
use bedtime_ui::prelude::*;
use bedtime_ui::stories::{self, Knobs};

fn pass(mut draw: impl FnMut(&mut Ui)) {
    let mut output = egui::Context::default().run_ui(Default::default(), |ui| draw(ui));
    output.textures_delta.clear();
}

#[test]
fn a_button_is_as_tall_as_its_size_and_wider_than_its_padding() {
    pass(|ui| {
        let mut last = 0.0;
        for size in Size::ALL {
            let response = ui.add(Button::new("Save").size(size));
            assert_eq!(response.rect.height(), size.height(ui));
            assert!(response.rect.width() > 2.0 * size.padding(ui).x);
            assert!(response.rect.height() > last, "sizes grow");
            last = response.rect.height();
        }
        // An icon alone is square.
        let icon = ui.add(Button::icon_only(bedtime_ui::BuiltinIcon::Close, "Close"));
        assert_eq!(icon.rect.width(), icon.rect.height());
    });
}

#[test]
fn a_button_fills_the_width_when_asked() {
    pass(|ui| {
        let available = ui.available_width();
        let response = ui.add(Button::new("Save").fill_width());
        assert_eq!(response.rect.width(), available);
    });
}

#[test]
fn space_is_steps_of_a_quarter_of_the_font_size() {
    pass(|ui| {
        let points: Vec<f32> = Space::ALL.iter().map(|space| space.px(ui)).collect();
        let step = points[1];
        assert!(step > 0.0);
        assert_eq!(
            points,
            [0.0, step, 2.0 * step, 3.0 * step, 4.0 * step, 6.0 * step]
        );
    });
}

#[test]
fn a_row_puts_things_side_by_side_a_gap_apart() {
    pass(|ui| {
        let gap = Space::Md.px(ui);
        Row::new().gap(Space::Md).show(ui, |ui| {
            let first = ui.add(Button::new("One")).rect;
            let second = ui.add(Button::new("Two")).rect;
            assert_eq!(second.left() - first.right(), gap);
            assert_eq!(first.center().y, second.center().y);
        });
    });
}

#[test]
fn a_column_puts_things_one_under_another_a_gap_apart() {
    pass(|ui| {
        let gap = Space::Lg.px(ui);
        Column::new().gap(Space::Lg).show(ui, |ui| {
            let first = ui.add(Button::new("One")).rect;
            let second = ui.add(Button::new("Two")).rect;
            assert_eq!(second.top() - first.bottom(), gap);
            assert_eq!(first.left(), second.left());
        });
    });
}

#[test]
fn knobs_keep_their_values_and_list_what_a_story_asked_for() {
    let mut knobs = Knobs::default();
    knobs.begin();
    assert!(!knobs.toggle("Disabled", false));
    assert_eq!(knobs.choice("Size", &Size::ALL), Size::Sm);
    assert_eq!(knobs.text("Label", "Save"), "Save");
    // Asked again, the same.
    knobs.begin();
    assert!(!knobs.toggle("Disabled", true), "the first value is kept");
}

#[test]
fn every_story_draws() {
    for story in stories::builtin() {
        let mut knobs = Knobs::default();
        for _ in 0..2 {
            pass(|ui| {
                knobs.begin();
                (story.draw)(ui, &mut knobs);
                knobs.controls(ui);
            });
        }
    }
}

/// Draw `draw` in `ctx`, with a click at `at` (press on one pass, release on
/// the next) if there is one.
fn passes(ctx: &egui::Context, at: Option<egui::Pos2>, mut draw: impl FnMut(&mut Ui)) {
    let pointer = |pressed| {
        at.map(|pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        })
    };
    let steps = [
        at.map(egui::Event::PointerMoved).into_iter().collect(),
        pointer(true).into_iter().collect(),
        pointer(false).into_iter().collect(),
        Vec::new(),
    ];
    for events in steps {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| draw(ui));
        output.textures_delta.clear();
    }
}

#[test]
fn a_click_ticks_a_checkbox_and_flips_a_toggle() {
    let ctx = egui::Context::default();
    let (mut ticked, mut on) = (false, false);
    let mut centers = [egui::Pos2::ZERO; 2];
    passes(&ctx, None, |ui| {
        centers[0] = ui.add(Checkbox::new(&mut ticked, "Wrap")).rect.center();
        centers[1] = ui.add(Toggle::new(&mut on, "Autosave")).rect.center();
    });
    let mut changes = 0;
    for (index, expected) in [(0, (true, false)), (1, (true, true)), (0, (false, true))] {
        passes(&ctx, Some(centers[index]), |ui| {
            changes += usize::from(ui.add(Checkbox::new(&mut ticked, "Wrap")).changed());
            changes += usize::from(ui.add(Toggle::new(&mut on, "Autosave")).changed());
        });
        assert_eq!((ticked, on), expected);
    }
    assert_eq!(changes, 3, "each click is one change");
}

#[test]
fn a_text_field_is_one_control_tall_and_as_wide_as_asked() {
    pass(|ui| {
        let mut text = String::from("hello");
        for size in Size::ALL {
            let top = ui.cursor().top();
            ui.add(TextField::new(&mut text).size(size).width(200.0));
            let spacing = ui.spacing().item_spacing.y;
            assert_eq!(ui.cursor().top() - top - spacing, size.height(ui));
        }
        let before = ui.min_rect().width();
        ui.add(TextField::new(&mut text).fill_width());
        assert!(ui.min_rect().width() >= before);
        assert_eq!(text, "hello", "drawing doesn't change the text");
    });
}

#[test]
fn edit_returns_a_new_value_only_when_the_control_changed_it() {
    pass(|ui| {
        let state = String::from("read-only");
        assert_eq!(
            ui.edit(&state, |ui, text| ui.add(TextField::new(text))),
            None
        );
        let edited = ui.edit(&state, |_, text| text.push('!'));
        assert_eq!(edited.as_deref(), Some("read-only!"));
        assert_eq!(state, "read-only");
    });
}

#[test]
fn a_select_opens_on_a_click_and_the_keys_choose_an_option() {
    const OPTIONS: [&str; 3] = ["GET", "POST", "DELETE"];
    let ctx = egui::Context::default();
    let mut value = "GET";
    let mut center = egui::Pos2::ZERO;
    let mut height = 0.0;
    passes(&ctx, None, |ui| {
        let rect = ui.add(Select::new(&mut value, &OPTIONS)).rect;
        (center, height) = (rect.center(), rect.height());
    });
    assert_eq!(height, {
        let mut expected = 0.0;
        pass(|ui| expected = Size::Md.height(ui));
        expected
    });
    // A click opens it.
    passes(&ctx, Some(center), |ui| {
        ui.add(Select::new(&mut value, &OPTIONS));
    });
    assert!(egui::Popup::is_any_open(&ctx), "a click opens the list");
    // Down twice from GET, then Enter.
    let mut changed = false;
    for key in [egui::Key::ArrowDown, egui::Key::ArrowDown, egui::Key::Enter] {
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            changed |= ui.add(Select::new(&mut value, &OPTIONS)).changed();
        });
        output.textures_delta.clear();
    }
    assert_eq!(value, "DELETE");
    assert!(changed);
    assert!(!egui::Popup::is_any_open(&ctx), "choosing closes the list");
}
