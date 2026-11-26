//! Components: widgets drawn from the theme and the tokens, added to any
//! `egui::Ui` (`ui.add(Button::new("Save").primary())`).
mod breadcrumbs;
mod button;
mod checkbox;
mod color_field;
mod edit;
mod list;
mod number_field;
mod section;
mod segmented;
mod select;
mod slider;
mod spinner;
mod stack;
mod table;
mod text;
mod text_area;
mod text_field;
mod tip;

pub use breadcrumbs::Breadcrumbs;
pub use button::{Button, Variant};
pub use checkbox::{Checkbox, Toggle};
pub use color_field::{ColorField, from_hex, to_hex};
pub use edit::UiEdit;
pub use list::{ListRow, MenuButton, MenuItem, Submenu};
pub use number_field::NumberField;
pub use section::{Section, Separator};
pub use segmented::Segmented;
pub use select::Select;
pub use slider::Slider;
pub use spinner::Spinner;
pub use stack::{Column, Row};
pub use table::{Table, TableRows};
pub use text::{Link, Text};
pub use text_area::TextArea;
pub use text_field::TextField;
pub use tip::Tip;

/// The width a widget that fills its space takes: all there is, except while
/// what it's in is being measured (a menu, a popup), when taking it all
/// would make that as wide as the window. Then, `otherwise`.
pub(crate) fn fill_width(ui: &egui::Ui, otherwise: f32) -> f32 {
    if ui.is_sizing_pass() {
        otherwise
    } else {
        ui.available_width()
    }
}
