//! Native Markdown previews of the host's shared text documents.
mod ecs;
pub use ecs::{MarkdownDocument, MarkdownPlugin, MarkdownStates, MarkdownSystems};

mod assets;
mod gpu;
mod layout;
mod model;
mod panel;
mod worker;

use bed_editor_api::{DocumentKind, Registrar};
pub use panel::MarkdownPanel;

pub const PLUGIN_ID: &str = "bed.markdown";
pub const PANEL_ID: &str = "bed.markdown.panel";
pub const VIEWER_ID: &str = "bed.markdown.viewer";
pub const OPEN_COMMAND: &str = "bed.markdown.open";
pub const OPEN_FILE_COMMAND: &str = "bed.markdown.open-file";
pub const SUPPORTED_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

fn register_metadata(r: &mut Registrar<'_>) {
    r.panel(PANEL_ID, "Markdown Preview");
    r.panel_requires_input(PANEL_ID);
    r.viewer(
        VIEWER_ID,
        "Markdown Preview",
        PANEL_ID,
        SUPPORTED_EXTENSIONS,
        DocumentKind::Text,
    );
    r.command(OPEN_COMMAND, "Open Markdown…", None);
    r.command(OPEN_FILE_COMMAND, "Open in Markdown Preview", None);
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
