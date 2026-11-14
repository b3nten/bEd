//! The editor in the status bar (`add_panel_status::<EditorPanel>`): where
//! the caret is, or how much is selected, and the text's language.
use super::{EditorPanel, TextSelection};
use bedtime_document::TextBuffer;
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::{DocumentReference, WorkbenchWorldExt, status::StatusPieces};
use bevy::prelude::*;

/// Selections longer than this (in bytes) are counted in lines, not
/// characters: the count is redone each frame the bar draws.
const COUNT_CHARACTERS_UP_TO: usize = 1 << 20;

/// The status bar item for an editor panel.
pub fn item(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>, panel: Entity) {
    let Some(editor) = world.get::<EditorPanel>(panel) else {
        return;
    };
    let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) else {
        return;
    };
    let workbench = world.workbench();
    let Some(text) = workbench
        .cast_document::<TextBuffer>(document)
        .and_then(|handle| workbench.read_document(handle))
        .ok()
    else {
        return;
    };
    // Right to left: the language, then the position.
    let mut pieces = StatusPieces::new(ui, context);
    pieces.weak(language_label(editor.view.language));
    pieces.weak(position(
        text,
        editor.view.selection(),
        editor.view.caret_count(),
    ));
}

/// "Ln 12, Col 4", with what's selected after it, or how many carets.
fn position(text: &TextBuffer, selection: TextSelection, carets: usize) -> String {
    // The selection may be a frame behind an edit: keep it inside the text.
    let head = text.clamp_boundary(selection.head().min(text.len()));
    let line = text.line_of(head) + 1;
    let column = text.column(head) + 1;
    let mut position = format!("Ln {line}, Col {column}");
    if carets > 1 {
        position.push_str(&format!(" ({carets} carets)"));
        return position;
    }
    let range = selection.range();
    let range = text.clamp_boundary(range.start.min(text.len()))
        ..text.clamp_boundary(range.end.min(text.len()));
    if range.is_empty() {
        return position;
    }
    let selected = if text.is_ascii() {
        plural(range.len(), "character")
    } else if range.len() <= COUNT_CHARACTERS_UP_TO {
        plural(text.range_text(range).chars().count(), "character")
    } else {
        plural(
            text.line_of(range.end) - text.line_of(range.start) + 1,
            "line",
        )
    };
    position.push_str(&format!(" ({selected} selected)"));
    position
}

fn plural(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

/// How a language id (`bedtime_syntax::LANGUAGES`) reads; no language is
/// plain text.
fn language_label(language: Option<&str>) -> &str {
    let Some(language) = language else {
        return "Plain Text";
    };
    match language {
        "rust" => "Rust",
        "c" => "C",
        "cpp" => "C++",
        "c-sharp" => "C#",
        "java" => "Java",
        "go" => "Go",
        "swift" => "Swift",
        "kotlin" => "Kotlin",
        "dart" => "Dart",
        "python" => "Python",
        "ruby" => "Ruby",
        "php" => "PHP",
        "lua" => "Lua",
        "bash" => "Shell",
        "powershell" => "PowerShell",
        "javascript" => "JavaScript",
        "typescript" => "TypeScript",
        "tsx" => "TSX",
        "html" => "HTML",
        "css" => "CSS",
        "json" => "JSON",
        "yaml" => "YAML",
        "toml" => "TOML",
        "xml" => "XML",
        "markdown" => "Markdown",
        "sql" => "SQL",
        "dockerfile" => "Dockerfile",
        "cmake" => "CMake",
        "make" => "Makefile",
        "hcl" => "HCL",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &TextBuffer, anchor: usize, head: usize) -> TextSelection {
        TextSelection::new(text, anchor, head).unwrap()
    }

    #[test]
    fn the_position_is_one_based_with_what_is_selected() {
        let text = TextBuffer::from("fn main() {\n    let é = 1;\n}\n");
        assert_eq!(position(&text, at(&text, 0, 0), 1), "Ln 1, Col 1");
        assert_eq!(position(&text, at(&text, 16, 16), 1), "Ln 2, Col 5");
        assert_eq!(
            position(&text, at(&text, 12, 22), 1),
            "Ln 2, Col 10 (9 characters selected)",
            "é is one character, two bytes"
        );
        assert_eq!(
            position(&text, at(&text, 3, 2), 1),
            "Ln 1, Col 3 (1 character selected)"
        );
        assert_eq!(
            position(&text, at(&text, 0, 0), 3),
            "Ln 1, Col 1 (3 carets)"
        );
    }

    #[test]
    fn a_stale_selection_past_the_end_is_kept_inside_the_text() {
        let text = TextBuffer::from("ab");
        let stale = at(&TextBuffer::from("abcdef"), 6, 6);
        assert_eq!(position(&text, stale, 1), "Ln 1, Col 3");
    }

    #[test]
    fn languages_read_as_names() {
        assert_eq!(language_label(Some("c-sharp")), "C#");
        assert_eq!(language_label(None), "Plain Text");
        for &language in bedtime_syntax::LANGUAGES {
            assert_ne!(
                language_label(Some(language)),
                language,
                "{language} has a label"
            );
        }
    }
}
