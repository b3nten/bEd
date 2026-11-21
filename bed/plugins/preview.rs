//! Previews: Markdown rendered and JSON/TOML as trees, viewers like any
//! other. Open to the Side shows a document's other viewer beside it (and
//! back).
use bedtime_datatree::DataTreePlugin;
use bedtime_markdown::MarkdownPlugin;
use bevy::prelude::*;

pub struct BedPreviewPlugin;

impl Plugin for BedPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((MarkdownPlugin, DataTreePlugin));
    }
}
