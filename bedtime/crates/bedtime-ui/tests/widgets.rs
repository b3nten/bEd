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
