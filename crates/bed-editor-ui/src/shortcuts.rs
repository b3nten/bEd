//! Editor commands share one keymap in standalone and embedded widgets.
use bed_keybindings::{Action, Bindings, EDITOR_SCOPE, Overrides, Platform, Scope, ShortcutInput};

pub fn actions() -> Vec<Action> {
    let scope = Scope::panel(EDITOR_SCOPE);
    let mut result = bed_keybindings::common_actions();
    for (id, label, keys) in [
        ("bed.app.Save", "Save", vec!["Primary+S"]),
        ("bed.app.SaveAs", "Save As", vec!["Primary+Shift+S"]),
        ("bed.app.OpenFile", "Open File", vec!["Primary+O"]),
    ] {
        result.push(Action::new(id, label, "bed.app", Scope::Focused, &keys));
    }
    let mut add = |name: &str, label: &str, keys: &[&str], repeat| {
        let mut action = Action::new(
            &format!("bed.editor.{name}"),
            label,
            "bed.editor",
            scope.clone(),
            keys,
        );
        action.repeat = repeat;
        result.push(action);
    };
    for (name, label, keys) in [
        ("go_to_line", "Go to Line", vec!["Ctrl+G"]),
        ("collapse_selection", "Collapse Selection", vec!["Escape"]),
        (
            "toggle_comment",
            "Toggle Line Comment",
            vec!["Primary+Slash"],
        ),
        (
            "add_cursor_above",
            "Add Cursor Above",
            vec!["Primary+Alt+Up"],
        ),
        (
            "add_cursor_below",
            "Add Cursor Below",
            vec!["Primary+Alt+Down"],
        ),
        ("definition", "Go to Definition", vec!["F12"]),
        ("references", "Find References", vec!["Shift+F12"]),
        ("symbol_info", "Show Symbol Information", vec![]),
        ("completion", "Complete", vec!["Ctrl+Space"]),
        (
            "format_document",
            "Format Document",
            vec!["Primary+Shift+I"],
        ),
        ("rename", "Rename Symbol", vec!["F2"]),
        ("quick_fix", "Quick Fix", vec!["Primary+Period"]),
        (
            "toggle_fold",
            "Toggle Fold",
            if cfg!(target_os = "macos") {
                vec!["Primary+Alt+LeftBracket"]
            } else {
                vec!["Ctrl+Shift+LeftBracket"]
            },
        ),
        ("fold_all", "Fold All", vec![]),
        ("unfold_all", "Unfold All", vec![]),
        ("zoom_in", "Zoom Text In", vec![]),
        ("zoom_out", "Zoom Text Out", vec![]),
        ("reset_zoom", "Reset Text Zoom", vec![]),
        ("toggle_wrap", "Toggle Word Wrap", vec!["Alt+Z"]),
        ("toggle_minimap", "Toggle Minimap", vec![]),
    ] {
        add(name, label, &keys, false);
    }
    for (name, label, keys) in [
        ("newline", "Insert Newline", vec!["Enter", "KeypadEnter"]),
        ("indent", "Indent", vec!["Tab", "Primary+RightBracket"]),
        (
            "outdent",
            "Outdent",
            vec!["Shift+Tab", "Primary+LeftBracket"],
        ),
        ("delete_left", "Delete Left", vec!["Backspace"]),
        ("delete_right", "Delete Right", vec!["Delete"]),
        (
            "delete_word_left",
            "Delete Previous Word",
            vec![if cfg!(target_os = "macos") {
                "Alt+Backspace"
            } else {
                "Ctrl+Backspace"
            }],
        ),
        (
            "delete_word_right",
            "Delete Next Word",
            vec![if cfg!(target_os = "macos") {
                "Alt+Delete"
            } else {
                "Ctrl+Delete"
            }],
        ),
    ] {
        add(name, label, &keys, true);
    }
    for (name, label, keys) in [
        (
            "word_left",
            "Previous Word",
            vec![if cfg!(target_os = "macos") {
                "Alt+Left"
            } else {
                "Ctrl+Left"
            }],
        ),
        (
            "word_right",
            "Next Word",
            vec![if cfg!(target_os = "macos") {
                "Alt+Right"
            } else {
                "Ctrl+Right"
            }],
        ),
        (
            "line_start",
            "Line Start",
            if cfg!(target_os = "macos") {
                vec!["Home", "Command+Left"]
            } else {
                vec!["Home"]
            },
        ),
        (
            "line_end",
            "Line End",
            if cfg!(target_os = "macos") {
                vec!["End", "Command+Right"]
            } else {
                vec!["End"]
            },
        ),
        (
            "doc_start",
            "Document Start",
            if cfg!(target_os = "macos") {
                vec!["Command+Up", "Ctrl+Home"]
            } else {
                vec!["Ctrl+Home"]
            },
        ),
        (
            "doc_end",
            "Document End",
            if cfg!(target_os = "macos") {
                vec!["Command+Down", "Ctrl+End"]
            } else {
                vec!["Ctrl+End"]
            },
        ),
        ("page_up", "Page Up", vec!["PageUp"]),
        ("page_down", "Page Down", vec!["PageDown"]),
    ] {
        add(
            &format!("move_{name}"),
            &format!("Move to {label}"),
            &keys,
            true,
        );
        let shifted = keys
            .iter()
            .map(|key| format!("Shift+{key}"))
            .collect::<Vec<_>>();
        add(
            &format!("select_{name}"),
            &format!("Select to {label}"),
            &shifted.iter().map(String::as_str).collect::<Vec<_>>(),
            true,
        );
    }
    result
}

pub fn default_input() -> ShortcutInput {
    ShortcutInput::new(Bindings::new(
        actions(),
        &Overrides::default(),
        Platform::current(),
    ))
    .with_scope(Scope::panel(EDITOR_SCOPE))
}

pub fn hex_actions() -> Vec<Action> {
    vec![Action::new(
        "bed.editor.hex_insert",
        "Toggle Insert Mode",
        "bed.editor",
        Scope::panel("bed.editor.hex-panel"),
        &["Insert"],
    )]
}

pub fn hex_default_input() -> ShortcutInput {
    let mut declarations = actions();
    declarations.extend(hex_actions());
    ShortcutInput::new(Bindings::new(
        declarations,
        &Default::default(),
        Platform::current(),
    ))
    .with_scope(Scope::panel(EDITOR_SCOPE))
}
