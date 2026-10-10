//! Bed core components. Attribution: workspace LICENSE and NOTICE.
pub mod buffer;
pub mod diagnostic;
pub mod editor_commands;
pub mod editor_events;
pub mod editor_operations;
pub mod editor_state;
pub mod editor_view_state;
pub mod identity;
pub mod project_undo;
pub mod text_search;
pub mod util;

pub use editor_state::DocumentKind;
