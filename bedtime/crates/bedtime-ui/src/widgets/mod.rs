//! Components: widgets drawn from the theme and the tokens, added to any
//! `egui::Ui` (`ui.add(Button::new("Save").primary())`).
mod button;
mod checkbox;
mod edit;
mod section;
mod segmented;
mod select;
mod stack;
mod text;
mod text_field;

pub use button::{Button, Variant};
pub use checkbox::{Checkbox, Toggle};
pub use edit::UiEdit;
pub use section::{Section, Separator};
pub use segmented::Segmented;
pub use select::Select;
pub use stack::{Column, Row};
pub use text::Text;
pub use text_field::TextField;
