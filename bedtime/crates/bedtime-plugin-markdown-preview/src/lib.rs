//! Native Markdown previews of the host's shared text documents.
mod ecs;
pub use ecs::{MarkdownDocument, MarkdownPreviewPlugin, MarkdownStates, MarkdownSystems};

mod assets;
mod gpu;
mod layout;
mod model;
mod panel;
mod worker;

use bedtime_api::{documents::DocumentKind, features::FeatureRegistrar};
pub use panel::MarkdownPanel;

pub const PLUGIN_ID: &str = "bedtime.markdown";
pub const PANEL_ID: &str = "bedtime.markdown.panel";
pub const VIEWER_ID: &str = "bedtime.markdown.viewer";
pub const OPEN_COMMAND: &str = "bedtime.markdown.open";
pub const OPEN_FILE_COMMAND: &str = "bedtime.markdown.open-file";
pub const SUPPORTED_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

fn register_metadata(r: &mut FeatureRegistrar<'_>) {
    r.panel_ref(PANEL_ID);
    r.panel_requires_input(PANEL_ID);
    r.viewer(
        VIEWER_ID,
        "Markdown Preview",
        PANEL_ID,
        SUPPORTED_EXTENSIONS,
        DocumentKind::Text,
    );
    r.command_ref(OPEN_COMMAND);
    r.command_ref(OPEN_FILE_COMMAND);
}

fn supported_path(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, ext)| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|value| value.eq_ignore_ascii_case(ext))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions_are_case_insensitive() {
        for path in ["README.MD", "a.markdown", "a.mdown", "a.MkD"] {
            assert!(supported_path(path));
        }
        for path in ["README", "a.json", "file.md/image.png"] {
            assert!(!supported_path(path));
        }
    }
}
