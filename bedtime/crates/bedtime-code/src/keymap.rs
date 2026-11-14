//! Keys for the code actions, as keymaps an app can choose with
//! `add_keymap` (the actions themselves have none).
use bedtime_workbench::{KeyChord, Keymap, Modifiers};
use bevy::input::keyboard::KeyCode;

const fn keys(modifiers: Modifiers, key: KeyCode) -> KeyChord {
    KeyChord::new(modifiers, key)
}
const NONE: Modifiers = Modifiers::NONE;
const PRIMARY: Modifiers = Modifiers::PRIMARY;
const ALT: Modifiers = Modifiers::ALT;
const SHIFT: Modifiers = Modifiers::SHIFT;
const PRIMARY_ALT: Modifiers = PRIMARY.union(ALT);
const PRIMARY_SHIFT: Modifiers = PRIMARY.union(SHIFT);
const SHIFT_ALT: Modifiers = SHIFT.union(ALT);
/// Expand/Shrink Selection: Ctrl+Shift+Cmd on macOS, where Shift+Alt+arrows
/// select by word.
const EXPAND: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::CONTROL.union(SHIFT).union(Modifiers::META)
} else {
    SHIFT_ALT
};
/// Fold/Unfold: Cmd+Alt on macOS, Ctrl+Shift elsewhere.
const FOLD: Modifiers = if cfg!(target_os = "macos") {
    PRIMARY_ALT
} else {
    PRIMARY_SHIFT
};

/// VS Code's keys, where they can be had. Alt with a letter types a
/// character on macOS, so Add Cursors to Line Ends is Cmd/Ctrl+Alt+I (not
/// Shift+Alt+I) and Toggle Block Comment Cmd/Ctrl+Shift+/ (not Shift+Alt+A).
/// Toggle Fold, Fold All and Unfold All have none: VS Code's are two-key
/// sequences.
pub const VSCODE: &Keymap = &[
    // Selection.
    ("editor.select_line", &[keys(PRIMARY, KeyCode::KeyL)]),
    (
        "editor.add_caret_above",
        &[keys(PRIMARY_ALT, KeyCode::ArrowUp)],
    ),
    (
        "editor.add_caret_below",
        &[keys(PRIMARY_ALT, KeyCode::ArrowDown)],
    ),
    (
        "editor.add_next_occurrence",
        &[keys(PRIMARY, KeyCode::KeyD)],
    ),
    (
        "editor.select_all_occurrences",
        &[keys(PRIMARY_SHIFT, KeyCode::KeyL)],
    ),
    (
        "editor.add_carets_to_line_ends",
        &[keys(PRIMARY_ALT, KeyCode::KeyI)],
    ),
    (
        "editor.expand_selection",
        &[keys(EXPAND, KeyCode::ArrowRight)],
    ),
    (
        "editor.shrink_selection",
        &[keys(EXPAND, KeyCode::ArrowLeft)],
    ),
    (
        "editor.jump_to_bracket",
        &[keys(PRIMARY_SHIFT, KeyCode::Backslash)],
    ),
    // Lines.
    ("editor.delete_line", &[keys(PRIMARY_SHIFT, KeyCode::KeyK)]),
    ("editor.copy_line_up", &[keys(SHIFT_ALT, KeyCode::ArrowUp)]),
    (
        "editor.copy_line_down",
        &[keys(SHIFT_ALT, KeyCode::ArrowDown)],
    ),
    ("editor.move_line_up", &[keys(ALT, KeyCode::ArrowUp)]),
    ("editor.move_line_down", &[keys(ALT, KeyCode::ArrowDown)]),
    (
        "editor.insert_line_above",
        &[keys(PRIMARY_SHIFT, KeyCode::Enter)],
    ),
    ("editor.insert_line_below", &[keys(PRIMARY, KeyCode::Enter)]),
    (
        "editor.join_lines",
        &[keys(Modifiers::CONTROL, KeyCode::KeyJ)],
    ),
    (
        "editor.indent_lines",
        &[keys(PRIMARY, KeyCode::BracketRight)],
    ),
    (
        "editor.outdent_lines",
        &[keys(PRIMARY, KeyCode::BracketLeft)],
    ),
    (
        "editor.toggle_line_comment",
        &[keys(PRIMARY, KeyCode::Slash)],
    ),
    (
        "editor.toggle_block_comment",
        &[keys(PRIMARY_SHIFT, KeyCode::Slash)],
    ),
    // Folding.
    ("editor.fold", &[keys(FOLD, KeyCode::BracketLeft)]),
    ("editor.unfold", &[keys(FOLD, KeyCode::BracketRight)]),
    // Cmd/Ctrl+Alt+L: Alt+letter types a character on macOS.
    (
        "editor.toggle_word_wrap",
        &[keys(PRIMARY_ALT, KeyCode::KeyL)],
    ),
    (
        "editor.format_and_save",
        &[keys(PRIMARY_ALT, KeyCode::KeyS)],
    ),
    // Language servers.
    ("code.go_to_definition", &[keys(NONE, KeyCode::F12)]),
    ("code.find_references", &[keys(SHIFT, KeyCode::F12)]),
    ("code.rename", &[keys(NONE, KeyCode::F2)]),
    ("code.actions", &[keys(PRIMARY, KeyCode::Period)]),
    // Not Alt-based: Alt with a letter types a character while editing.
    ("code.format", &[keys(PRIMARY_SHIFT, KeyCode::KeyI)]),
    // Structure.
    ("code.go_to_symbol", &[keys(PRIMARY_SHIFT, KeyCode::KeyO)]),
];
