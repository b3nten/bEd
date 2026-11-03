//! Viewers for text documents: Markdown Preview renders Markdown, Tree View
//! shows JSON and TOML as a tree. Both show the same `TextBuffer` document
//! an editor does, so they follow its edits, unsaved too. Neither is the
//! default for its files (the editor is); they're offered in "Open With",
//! and `preview_opener` names the one for a file.
use bedtime_code::{editor::text_fit, syntax::language_name};
use bedtime_document::TextBuffer;
use bedtime_workbench::{Fit, Opener, OpenerId, OpenersAppExt};
use bevy::prelude::*;
use std::path::Path;

pub mod data;
pub mod markdown;
pub use data::TreePanel;
pub use markdown::MarkdownPanel;

pub struct PreviewPlugin;

impl Plugin for PreviewPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<MarkdownPanel>()
            .register_type::<TreePanel>()
            .register_opener(
                Opener::<TextBuffer>::new::<MarkdownPanel>("Markdown Preview").files(markdown_fit),
            )
            .register_opener(Opener::<TextBuffer>::new::<TreePanel>("Tree View").files(tree_fit));
    }
}

fn is_markdown(name: &str) -> bool {
    language_name(name) == Some("markdown")
}

/// Text files named as Markdown; as good a fit as the editor, which stays
/// the default.
fn markdown_fit(path: &Path, head: &[u8]) -> Fit {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if is_markdown(&name) {
        text_fit(path, head)
    } else {
        Fit::No
    }
}

fn tree_fit(path: &Path, head: &[u8]) -> Fit {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if data::Format::of(&name).is_some() {
        text_fit(path, head)
    } else {
        Fit::No
    }
}

/// The viewer that previews a file named `name` (a document's title), if
/// there's one.
pub fn preview_opener(name: &str) -> Option<OpenerId> {
    if is_markdown(name) {
        Some(OpenerId::of::<MarkdownPanel>())
    } else if data::Format::of(name).is_some() {
        Some(OpenerId::of::<TreePanel>())
    } else {
        None
    }
}

/// Whether `panel` is one of these viewers.
pub fn is_preview(world: &World, panel: Entity) -> bool {
    world.get::<MarkdownPanel>(panel).is_some() || world.get::<TreePanel>(panel).is_some()
}
