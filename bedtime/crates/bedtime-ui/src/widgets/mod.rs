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

pub use breadcrumbs::Breadcrumbs;
pub use button::{Button, Variant};
pub use checkbox::{Checkbox, Toggle};
pub use color_field::{ColorField, from_hex, to_hex};
pub use edit::UiEdit;
pub use list::{ListRow, MenuItem, Submenu};
pub use number_field::NumberField;
pub use section::{Section, Separator};
pub use segmented::Segmented;
pub use select::Select;
pub use slider::Slider;
pub use spinner::Spinner;
pub use stack::{Column, Row};
pub use table::{Table, TableRows};
pub use text::Text;
pub use text_area::TextArea;
pub use text_field::TextField;
