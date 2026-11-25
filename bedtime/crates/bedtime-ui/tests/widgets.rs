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

#[test]
fn a_click_on_a_segment_chooses_it() {
    const OPTIONS: [&str; 3] = ["Day", "Week", "Month"];
    let ctx = egui::Context::default();
    let mut value = "Day";
    let mut rect = egui::Rect::NOTHING;
    passes(&ctx, None, |ui| {
        rect = ui.add(Segmented::new(&mut value, &OPTIONS)).rect;
    });
    let mut height = 0.0;
    pass(|ui| height = Size::Md.height(ui));
    assert_eq!(rect.height(), height, "one control tall");
    // The last segment is at the right end.
    let mut changes = 0;
    passes(
        &ctx,
        Some(rect.right_center() - egui::vec2(10.0, 0.0)),
        |ui| {
            changes += usize::from(ui.add(Segmented::new(&mut value, &OPTIONS)).changed());
        },
    );
    assert_eq!((value, changes), ("Month", 1));
}

#[test]
fn segments_wrap_when_they_do_not_fit() {
    const OPTIONS: [&str; 6] = ["Graph", "Lanes", "Arcs", "Timeline", "Flame", "List"];
    pass(|ui| {
        let mut value = "Graph";
        let one_row = ui.add(Segmented::new(&mut value, &OPTIONS)).rect;
        ui.scope(|ui| {
            ui.set_max_width(one_row.width() / 2.0);
            let wrapped = ui.add(Segmented::new(&mut value, &OPTIONS)).rect;
            assert!(wrapped.width() <= one_row.width() / 2.0 + 1.0);
            assert!(wrapped.height() > one_row.height());
        });
    });
}

#[test]
fn a_section_shows_its_content_only_while_open_and_a_click_toggles_it() {
    use std::cell::Cell;
    let ctx = egui::Context::default();
    let (header, shown) = (Cell::new(egui::Pos2::ZERO), Cell::new(false));
    let draw = |ui: &mut Ui| {
        header.set(ui.cursor().left_top() + egui::vec2(20.0, 10.0));
        let body = Section::new("Edges").show(ui, |ui| ui.add(Text::new("Ordering")));
        shown.set(body.is_some());
    };
    passes(&ctx, None, draw);
    assert!(!shown.get(), "closed until it's opened");
    passes(&ctx, Some(header.get()), draw);
    assert!(shown.get(), "a click on its header opens it");

    let mut open = false;
    pass(|ui| {
        open = Section::new("Frame").open().show(ui, |_| ()).is_some();
    });
    assert!(open, "open() starts open");
}

#[test]
fn a_separator_lies_across_a_column_and_stands_in_a_row() {
    pass(|ui| {
        let across = ui.add(Separator::new()).rect;
        assert!(across.width() > across.height());
        Row::new().show(ui, |ui| {
            ui.add(Button::new("Cut"));
            let upright = ui.add(Separator::new()).rect;
            assert!(upright.height() > upright.width());
        });
    });
}

#[test]
fn a_button_says_how_wide_it_will_be() {
    pass(|ui| {
        for button in [
            || Button::new("Cancel"),
            || Button::new("Send").primary().min_width(120.0),
            || {
                Button::new("New file")
                    .icon(bedtime_ui::BuiltinIcon::Plus)
                    .small()
            },
            || Button::icon_only(bedtime_ui::BuiltinIcon::Close, "Remove"),
        ] {
            let expected = button().width_in(ui);
            // egui snaps what it places to pixels.
            assert!((ui.add(button()).rect.width() - expected).abs() < 0.5);
        }
    });
}

#[test]
fn a_named_text_field_keeps_its_text_for_scripts() {
    // What bed's automation reads: a field is found by its label, or with
    // none by its text.
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut text = String::from("127.0.0.1/users");
    let mut output = ctx.run_ui(Default::default(), |ui| {
        ui.add(TextField::new(&mut text).hint("https://example.com"));
        ui.add(TextField::new(&mut text).hint("Search").label("Address"));
    });
    output.textures_delta.clear();
    let update = output.platform_output.accesskit_update.expect("a tree");
    let fields: Vec<_> = update
        .nodes
        .iter()
        .filter(|(_, node)| node.value() == Some("127.0.0.1/users"))
        .map(|(_, node)| node.label().map(str::to_owned))
        .collect();
    assert!(fields.len() >= 2, "both keep their text");
    assert!(fields.contains(&Some("Address".to_owned())));
    assert!(
        fields
            .iter()
            .any(|label| label.as_deref() != Some("Address"))
    );
}

#[test]
fn a_slider_follows_a_click_and_stays_in_its_range() {
    let ctx = egui::Context::default();
    let mut value = 0.2_f32;
    let mut rect = egui::Rect::NOTHING;
    passes(&ctx, None, |ui| {
        rect = ui.add(Slider::new(&mut value, 0.0..=1.0).width(200.0)).rect;
    });
    assert_eq!(value, 0.2, "drawing doesn't change it");
    let mut changed = false;
    passes(&ctx, Some(rect.center()), |ui| {
        changed |= ui
            .add(Slider::new(&mut value, 0.0..=1.0).width(200.0))
            .changed();
    });
    assert!(
        changed && (value - 0.5).abs() < 0.02,
        "the middle is half: {value}"
    );
    passes(
        &ctx,
        Some(rect.right_center() + egui::vec2(-1.0, 0.0)),
        |ui| {
            ui.add(Slider::new(&mut value, 0.0..=1.0).width(200.0));
        },
    );
    assert_eq!(value, 1.0, "the end is the most");

    // Whole numbers stay whole; a logarithmic one's middle is the geometric mean.
    let (mut whole, mut log) = (1_i32, 1.0_f32);
    let mut rects = [egui::Rect::NOTHING; 2];
    let ctx = egui::Context::default();
    passes(&ctx, None, |ui| {
        rects[0] = ui.add(Slider::new(&mut whole, 1..=8).width(200.0)).rect;
        rects[1] = ui
            .add(
                Slider::new(&mut log, 0.25..=64.0)
                    .logarithmic()
                    .width(200.0),
            )
            .rect;
    });
    for index in 0..2 {
        passes(&ctx, Some(rects[index].center()), |ui| {
            ui.add(Slider::new(&mut whole, 1..=8).width(200.0));
            ui.add(
                Slider::new(&mut log, 0.25..=64.0)
                    .logarithmic()
                    .width(200.0),
            );
        });
    }
    assert!((4..=5).contains(&whole));
    assert!((log - 4.0).abs() < 0.3, "sqrt(0.25 * 64) is 4: {log}");
}

#[test]
fn a_list_row_fills_the_width_and_is_clicked() {
    let ctx = egui::Context::default();
    let mut rect = egui::Rect::NOTHING;
    let mut clicks = 0;
    let mut available = 0.0;
    passes(&ctx, None, |ui| {
        available = ui.available_width();
        rect = ui
            .add(ListRow::new("Create a user").busy(true).trailing("3"))
            .rect;
    });
    assert_eq!(rect.width(), available);
    passes(&ctx, Some(rect.center()), |ui| {
        let row = ListRow::new("Create a user").tag("POST", egui::Color32::RED, "DELETE");
        clicks += usize::from(ui.add(row).clicked());
    });
    assert_eq!(clicks, 1);
}

#[test]
fn a_text_area_starts_as_tall_as_its_rows() {
    pass(|ui| {
        let mut text = String::new();
        let short = ui.add(TextArea::new(&mut text).rows(2)).rect.height();
        let tall = ui.add(TextArea::new(&mut text).rows(8)).rect.height();
        assert!(tall > short * 2.0);
        let wide = ui.add(TextArea::new(&mut text).fill_width()).rect.width();
        assert!((wide - ui.available_width()).abs() < 1.0);
    });
}

#[test]
fn a_table_lines_its_cells_up_in_columns() {
    pass(|ui| {
        let mut lefts: Vec<[f32; 2]> = Vec::new();
        let mut tops = Vec::new();
        Table::new("columns")
            .headers(["Column", "Type"])
            .show(ui, |table| {
                for (name, declared) in [("id", "INTEGER"), ("a_longer_name", "TEXT")] {
                    table.row(|ui| {
                        let first = ui.add(Text::new(name)).rect;
                        let second = ui.add(Text::new(declared)).rect;
                        lefts.push([first.left(), second.left()]);
                        tops.push(first.top());
                    });
                }
            });
        assert_eq!(
            lefts[0], lefts[1],
            "each column starts where it does in every row"
        );
        assert!(lefts[0][1] > lefts[0][0]);
        assert!(tops[1] > tops[0], "rows are one under another");
    });
}

#[test]
fn a_text_area_fills_the_height_when_asked() {
    pass(|ui| {
        let mut text = String::new();
        ui.scope(|ui| {
            ui.set_max_height(300.0);
            let available = ui.available_height();
            let area = ui.add(TextArea::new(&mut text).rows(1).fill_height()).rect;
            assert!(
                (area.height() - available).abs() < 2.0,
                "{} of {available}",
                area.height()
            );
        });
    });
}

#[test]
fn a_table_that_knows_its_columns_gives_the_last_the_width_left() {
    // A grid sizes its columns from the frame before: draw a few.
    let ctx = egui::Context::default();
    let long = "a value long enough that it has to wrap ".repeat(8);
    let mut cell = egui::Rect::NOTHING;
    passes(&ctx, None, |ui| {
        ui.scope(|ui| {
            ui.set_max_width(400.0);
            Table::new("headers").columns(2).show(ui, |table| {
                table.row(|ui| {
                    ui.add(Text::new("server").strong());
                    cell = ui.add(Text::new(&long).wrap()).rect;
                });
            });
        });
    });
    assert!(
        cell.width() > 200.0,
        "wrapped in the room left: {}",
        cell.width()
    );
}

#[test]
fn breadcrumbs_drop_the_first_when_narrow_and_say_which_was_clicked() {
    let names = ["bed", "bedtime", "crates", "bedtime-ui", "src"];
    let ctx = egui::Context::default();
    let (mut wide, mut narrow) = (0.0, 0.0);
    passes(&ctx, None, |ui| {
        let before = ui.cursor().left();
        Row::new().show(ui, |ui| {
            let _ = Breadcrumbs::new(names).show(ui);
            wide = ui.min_rect().right() - before;
        });
        ui.scope(|ui| {
            ui.set_max_width(wide / 2.0);
            Row::new().show(ui, |ui| {
                let _ = Breadcrumbs::new(names).show(ui);
                narrow = ui.min_rect().right() - before;
            });
        });
    });
    assert!(
        narrow <= wide / 2.0 + 1.0,
        "they fit the room: {narrow} of {wide}"
    );

    // The last one is at the right end.
    use std::cell::Cell;
    let (clicked, end) = (Cell::new(None), Cell::new(egui::Pos2::ZERO));
    let ctx = egui::Context::default();
    let draw = |ui: &mut Ui| {
        Row::new().show(ui, |ui| {
            clicked.set(clicked.get().or(Breadcrumbs::new(names).show(ui)));
            end.set(ui.min_rect().right_center() - egui::vec2(4.0, 0.0));
        });
    };
    passes(&ctx, None, draw);
    assert_eq!(clicked.get(), None);
    passes(&ctx, Some(end.get()), draw);
    assert_eq!(clicked.get(), Some(names.len() - 1));
}

#[test]
fn a_separator_takes_no_length_while_its_container_is_measured() {
    // A menu is as wide as its widest item, not as wide as there's room.
    let ctx = egui::Context::default();
    let mut width = 0.0;
    passes(&ctx, None, |ui| {
        egui::Area::new(egui::Id::new("menu"))
            .sizing_pass(true)
            .show(ui.ctx(), |ui| {
                let _ = ui.add(MenuItem::new("Copy"));
                ui.add(Separator::new());
                let _ = ui.add(MenuItem::new("Show Ignored Files").checked(true));
                width = ui.min_rect().width();
            });
    });
    assert!(width > 0.0 && width < 400.0, "sized to its items: {width}");
}

/// Draw `draw` in `ctx` after `events` (keys and text, for what has focus).
fn typed(ctx: &egui::Context, events: Vec<egui::Event>, mut draw: impl FnMut(&mut Ui)) {
    let input = egui::RawInput {
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| draw(ui));
    output.textures_delta.clear();
}

#[test]
fn a_number_field_shows_the_value_and_follows_what_is_typed() {
    let ctx = egui::Context::default();
    let mut value = 15.5_f32;
    let mut changes = 0;
    let mut center = egui::Pos2::ZERO;
    fn field(value: &mut f32) -> NumberField<'_, f32> {
        NumberField::new(value)
            .range(8.0..=72.0)
            .decimals(1)
            .suffix(" pt")
    }
    // Shown, and clicked into, it doesn't change the value (15.5 exactly).
    passes(&ctx, None, |ui| {
        center = ui.add(field(&mut value)).rect.center()
    });
    passes(&ctx, Some(center), |ui| {
        changes += usize::from(ui.add(field(&mut value)).changed());
    });
    assert_eq!((value, changes), (15.5, 0));

    // Select all and type: the value follows, inside the range.
    let select_all = egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    for (events, expected) in [
        (
            vec![select_all.clone(), egui::Event::Text("20".into())],
            20.0,
        ),
        (vec![egui::Event::Text(".5".into())], 20.5),
        (
            vec![select_all.clone(), egui::Event::Text("abc".into())],
            20.5,
        ),
        (vec![select_all, egui::Event::Text("500".into())], 72.0),
    ] {
        typed(&ctx, events, |ui| {
            changes += usize::from(ui.add(field(&mut value)).changed());
        });
        assert_eq!(value, expected);
    }
    assert_eq!(changes, 3, "only a new number is a change");
}

#[test]
fn a_number_field_does_not_round_a_value_by_showing_it() {
    let ctx = egui::Context::default();
    let mut value = 0.123456_f64;
    let mut center = egui::Pos2::ZERO;
    passes(&ctx, None, |ui| {
        center = ui.add(NumberField::new(&mut value)).rect.center();
    });
    passes(&ctx, Some(center), |ui| {
        ui.add(NumberField::new(&mut value));
    });
    assert_eq!(value, 0.123456, "shown as 0.12, kept as it was");
}

#[test]
fn hex_colours_round_trip() {
    use bedtime_ui::widgets::{from_hex, to_hex};
    let color = from_hex("#7aa2f7").unwrap();
    assert_eq!(to_hex(&color), "#7aa2f7");
    assert_eq!(to_hex(&from_hex("28345780").unwrap()), "#28345780");
    assert!(from_hex("#12345").is_none());
    assert!(from_hex("#zzzzzz").is_none());
}

#[test]
fn a_colour_field_follows_typed_hex_and_only_then_changes() {
    let ctx = egui::Context::default();
    let mut color = from_rgb(0x7a, 0xa2, 0xf7);
    let mut changes = 0;
    let mut rect = egui::Rect::NOTHING;
    passes(&ctx, None, |ui| {
        rect = ui.add(ColorField::new(&mut color)).rect
    });
    assert!(
        rect.width() > 96.0,
        "room for the swatch and the hex: {}",
        rect.width()
    );
    // Click into the hex (the right part), select it all, type another.
    passes(
        &ctx,
        Some(rect.right_center() - egui::vec2(20.0, 0.0)),
        |ui| {
            changes += usize::from(ui.add(ColorField::new(&mut color)).changed());
        },
    );
    assert_eq!(changes, 0, "shown and clicked, it's the same colour");
    let select_all = egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    typed(
        &ctx,
        vec![select_all, egui::Event::Text("#ff0000".into())],
        |ui| {
            changes += usize::from(ui.add(ColorField::new(&mut color)).changed());
        },
    );
    assert_eq!((color, changes), (from_rgb(0xff, 0, 0), 1));
}

fn from_rgb(r: u8, g: u8, b: u8) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

#[test]
fn a_button_at_a_set_width_is_that_wide_whatever_its_text() {
    pass(|ui| {
        let long = ui
            .add(Button::new("Commit with Signoff, and more").width(120.0))
            .rect;
        let short = ui.add(Button::new("Go").width(120.0)).rect;
        assert_eq!((long.width(), short.width()), (120.0, 120.0));
        let arrow = ui.add(Button::new("main").dropdown()).rect;
        let plain = ui.add(Button::new("main")).rect;
        assert!(arrow.width() > plain.width(), "room for its arrow");
    });
}

#[test]
fn a_menu_button_opens_its_menu_on_a_click() {
    use std::cell::Cell;
    let ctx = egui::Context::default();
    let (center, drawn) = (Cell::new(egui::Pos2::ZERO), Cell::new(0));
    let draw = |ui: &mut Ui| {
        let button = MenuButton::new(Button::new("main").dropdown()).show(ui, |ui| {
            drawn.set(drawn.get() + 1);
            let _ = ui.add(MenuItem::new("develop"));
        });
        center.set(button.rect.center());
    };
    passes(&ctx, None, draw);
    assert_eq!(drawn.get(), 0, "closed until it's clicked");
    passes(&ctx, Some(center.get()), draw);
    assert!(drawn.get() > 0 && egui::Popup::is_any_open(&ctx));
}

#[test]
fn what_fills_its_width_does_not_stretch_a_popup_being_measured() {
    let ctx = egui::Context::default();
    let mut width = 0.0;
    passes(&ctx, None, |ui| {
        egui::Area::new(egui::Id::new("popup"))
            .sizing_pass(true)
            .show(ui.ctx(), |ui| {
                let mut text = String::new();
                ui.add(TextField::new(&mut text).fill_width());
                ui.add(Button::new("Create and switch").fill_width());
                ui.add(ListRow::new("develop"));
                width = ui.min_rect().width();
            });
    });
    assert!(
        width > 0.0 && width < 400.0,
        "sized to its content: {width}"
    );
}
