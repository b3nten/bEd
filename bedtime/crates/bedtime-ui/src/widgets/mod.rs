//! Components: widgets drawn from the theme and the tokens, added to any
//! `egui::Ui` (`ui.add(Button::new("Save").primary())`).
mod button;
mod checkbox;
mod edit;
mod list;
mod section;
mod segmented;
mod select;
mod slider;
mod spinner;
mod stack;
mod text;
mod text_area;
mod text_field;

pub use button::{Button, Variant};
pub use checkbox::{Checkbox, Toggle};
pub use edit::UiEdit;
pub use list::{ListRow, MenuItem};
pub use section::{Section, Separator};
pub use segmented::Segmented;
pub use select::Select;
pub use slider::Slider;
pub use spinner::Spinner;
pub use stack::{Column, Row};
pub use text::Text;
pub use text_area::TextArea;
pub use text_field::TextField;
