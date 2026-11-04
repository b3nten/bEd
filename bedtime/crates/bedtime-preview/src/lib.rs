//! Viewers for text documents: Markdown Preview renders Markdown, Tree View
//! shows JSON and TOML as a tree. Both show the same `TextBuffer` document
//! an editor does, so they follow its edits, unsaved too. Neither is the
//! default for its files (the editor is); they're offered in "Open With",
//! and `preview_viewer` names the one for a file.
use bedtime_code::{EditorPanel, syntax::language_name};
use bedtime_document::TextBuffer;
use bedtime_workbench::{Format, FormatId, FormatsAppExt, TEXT, ViewerId, ViewersAppExt};
use bevy::prelude::*;

pub mod data;
pub mod markdown;
pub use data::TreePanel;
pub use markdown::MarkdownPanel;

/// The formats these viewers show. They're bedtime-code's languages too
/// (formats are shared by id); the names and extensions here are what the
/// previews know them by.
pub const MARKDOWN: FormatId = FormatId("markdown");
pub const JSON: FormatId = FormatId("json");
pub const TOML: FormatId = FormatId("toml");

pub struct PreviewPlugin;

impl Plugin for PreviewPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<MarkdownPanel>()
            .register_type::<TreePanel>()
            .register_format(
                Format::new(MARKDOWN)
                    .extensions(["md", "markdown", "mdown", "mkd", "mkdn"])
                    .conforms_to([TEXT]),
            )
            .register_format(
                Format::new(JSON)
                    .extensions(["json", "geojson", "webmanifest"])
                    .conforms_to([TEXT]),
            )
            .register_format(
                Format::new(TOML)
                    .extensions(["toml"])
                    .names(["cargo.lock", "uv.lock", "poetry.lock"])
                    .conforms_to([TEXT]),
            )
            .register_viewer::<TextBuffer, MarkdownPanel>("Markdown Preview")
            .formats([MARKDOWN])
            .register_viewer::<TextBuffer, TreePanel>("Tree View")
            .formats([JSON, TOML]);
        // The editor stays these files' default; the previews are "Open
        // With" (and bed's Open Preview). An app can say otherwise.
        for format in [MARKDOWN, JSON, TOML] {
            app.set_default_viewer::<EditorPanel>(format);
        }
    }
}

fn is_markdown(name: &str) -> bool {
    language_name(name) == Some("markdown")
}

/// The viewer that previews a file named `name` (a document's title), if
/// there's one.
pub fn preview_viewer(name: &str) -> Option<ViewerId> {
    if is_markdown(name) {
        Some(ViewerId::of::<MarkdownPanel>())
    } else if data::Format::of(name).is_some() {
        Some(ViewerId::of::<TreePanel>())
    } else {
        None
    }
}

/// Whether `panel` is one of these viewers.
pub fn is_preview(world: &World, panel: Entity) -> bool {
    world.get::<MarkdownPanel>(panel).is_some() || world.get::<TreePanel>(panel).is_some()
}
