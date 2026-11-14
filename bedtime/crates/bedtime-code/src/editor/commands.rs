//! Editor commands: what the editor does besides typing (Select Line, Move
//! Line Up, Fold…), as `EditorCommand`s it runs on its next draw.
//!
//! Each has an action, so it can be bound, put in menus and found by name.
//! The actions are handled by editor panels themselves: each gets an
//! `ActionHandler` per action, available while its text has the keyboard,
//! that queues the command. The workbench delivers their keys (even keys a
//! text field would keep, such as Alt+Up) before the editor draws. The
//! actions have no keys of their own: `crate::keymap` offers some.
use super::*;
use bedtime_workbench::{
    Action, ActionAppExt, ActionHandler, ActionInvocation, ActionResult, Handler,
};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCommand {
    /// Select the caret's line, newline included; again, the next line too.
    SelectLine,
    /// A caret on the line above the primary one.
    AddCaretAbove,
    /// A caret on the line below the primary one.
    AddCaretBelow,
    /// Select the next occurrence of the selection too (first, the word at
    /// the caret).
    AddNextOccurrence,
    /// Select every occurrence of the selection (or the word at the caret).
    SelectAllOccurrences,
    /// A caret at the end of each selected line.
    AddCaretsToLineEnds,
    /// Delete the lines of each caret.
    DeleteLine,
    /// Copy the lines above themselves.
    CopyLineUp,
    /// Copy the lines below themselves, selecting the copy.
    CopyLineDown,
    /// Swap the lines with the one above.
    MoveLineUp,
    /// Swap the lines with the one below.
    MoveLineDown,
    /// A new line above, indented like the caret's.
    InsertLineAbove,
    /// A new line below, indented like the caret's.
    InsertLineBelow,
    /// Join the line with the next (a selection's lines together).
    JoinLines,
    /// Indent the lines, wherever the caret is in them.
    IndentLines,
    /// Outdent the lines.
    OutdentLines,
    /// Grow the selection to the word, then the syntax around it.
    ExpandSelection,
    /// Undo growing the selection.
    ShrinkSelection,
    /// Comment the lines (in their language), or uncomment them.
    ToggleLineComment,
    /// Wrap the selection in a block comment, or unwrap it.
    ToggleBlockComment,
    /// Each caret to the bracket matching the one beside it.
    JumpToBracket,
    /// Fold the innermost region around each caret.
    Fold,
    /// Unfold the region at each caret.
    Unfold,
    /// Fold or unfold at each caret.
    ToggleFold,
    FoldAll,
    UnfoldAll,
}

impl EditorCommand {
    /// Whether it changes the text: not run in a read-only editor.
    pub fn edits(self) -> bool {
        matches!(
            self,
            EditorCommand::DeleteLine
                | EditorCommand::CopyLineUp
                | EditorCommand::CopyLineDown
                | EditorCommand::MoveLineUp
                | EditorCommand::MoveLineDown
                | EditorCommand::InsertLineAbove
                | EditorCommand::InsertLineBelow
                | EditorCommand::JoinLines
                | EditorCommand::IndentLines
                | EditorCommand::OutdentLines
                | EditorCommand::ToggleLineComment
                | EditorCommand::ToggleBlockComment
        )
    }
}

/// An action that runs an editor command in the focused editor.
pub trait EditorAction: Action {
    const COMMAND: EditorCommand;
}

macro_rules! editor_actions {
    ($($(#[$doc:meta])* $name:ident, $key:literal, $label:literal, $command:ident;)*) => {
        $(
            $(#[$doc])*
            pub struct $name;
            impl Action for $name {
                const KEY: &'static str = $key;
                const LABEL: &'static str = $label;
                const REPEATS: bool = true;
            }
            impl EditorAction for $name {
                const COMMAND: EditorCommand = EditorCommand::$command;
            }
        )*
    };
}

editor_actions! {
    SelectLine, "editor.select_line", "Select Line", SelectLine;
    AddCaretAbove, "editor.add_caret_above", "Add Cursor Above", AddCaretAbove;
    AddCaretBelow, "editor.add_caret_below", "Add Cursor Below", AddCaretBelow;
    AddNextOccurrence, "editor.add_next_occurrence", "Add Next Occurrence", AddNextOccurrence;
    SelectAllOccurrences, "editor.select_all_occurrences", "Select All Occurrences", SelectAllOccurrences;
    AddCaretsToLineEnds, "editor.add_carets_to_line_ends", "Add Cursors to Line Ends", AddCaretsToLineEnds;
    DeleteLine, "editor.delete_line", "Delete Line", DeleteLine;
    CopyLineUp, "editor.copy_line_up", "Copy Line Up", CopyLineUp;
    CopyLineDown, "editor.copy_line_down", "Copy Line Down", CopyLineDown;
    MoveLineUp, "editor.move_line_up", "Move Line Up", MoveLineUp;
    MoveLineDown, "editor.move_line_down", "Move Line Down", MoveLineDown;
    InsertLineAbove, "editor.insert_line_above", "Insert Line Above", InsertLineAbove;
    InsertLineBelow, "editor.insert_line_below", "Insert Line Below", InsertLineBelow;
    JoinLines, "editor.join_lines", "Join Lines", JoinLines;
    IndentLines, "editor.indent_lines", "Indent Line", IndentLines;
    OutdentLines, "editor.outdent_lines", "Outdent Line", OutdentLines;
    ExpandSelection, "editor.expand_selection", "Expand Selection", ExpandSelection;
    ShrinkSelection, "editor.shrink_selection", "Shrink Selection", ShrinkSelection;
    ToggleLineComment, "editor.toggle_line_comment", "Toggle Line Comment", ToggleLineComment;
    ToggleBlockComment, "editor.toggle_block_comment", "Toggle Block Comment", ToggleBlockComment;
    JumpToBracket, "editor.jump_to_bracket", "Go to Bracket", JumpToBracket;
    Fold, "editor.fold", "Fold", Fold;
    Unfold, "editor.unfold", "Unfold", Unfold;
    ToggleFold, "editor.toggle_fold", "Toggle Fold", ToggleFold;
    FoldAll, "editor.fold_all", "Fold All", FoldAll;
    UnfoldAll, "editor.unfold_all", "Unfold All", UnfoldAll;
}

/// How each editor action is handled on a panel.
#[derive(Resource, Default)]
struct EditorActions(Vec<fn(&mut EntityCommands)>);

pub(crate) trait EditorCommandsAppExt {
    /// Make `A` an action of editor panels: each handles it (while its text
    /// has the keyboard) by running `A::COMMAND`.
    fn add_editor_command<A: EditorAction>(&mut self) -> &mut Self;
}

impl EditorCommandsAppExt for App {
    fn add_editor_command<A: EditorAction>(&mut self) -> &mut Self {
        if !self.world().contains_resource::<EditorActions>() {
            self.init_resource::<EditorActions>()
                .add_observer(handle_actions);
        }
        self.world_mut()
            .resource_mut::<EditorActions>()
            .0
            .push(|panel| {
                panel.insert(ActionHandler::<A>::new(
                    Handler::new(queue::<A>).available_when(available::<A>),
                ));
            });
        self.register_action::<A>()
    }
}

/// A new editor panel handles the editor actions.
fn handle_actions(add: On<Add<EditorPanel>>, actions: Res<EditorActions>, mut commands: Commands) {
    let mut panel = commands.entity(add.entity);
    for handle in &actions.0 {
        handle(&mut panel);
    }
}

/// While the editor's text has the keyboard (not its Find field, say), and
/// for commands that edit, while it may be edited.
fn available<A: EditorAction>(world: &World, panel: Option<Entity>) -> bool {
    panel
        .and_then(|panel| world.get::<EditorPanel>(panel))
        .is_some_and(|editor| {
            editor.view.has_keyboard() && !(A::COMMAND.edits() && editor.view.settings.read_only)
        })
}

fn queue<A: EditorAction>(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let mut editor = invocation
        .entity
        .and_then(|panel| world.get_mut::<EditorPanel>(panel))
        .ok_or("No editor is focused")?;
    editor.view.queue_command(A::COMMAND);
    Ok(())
}

impl EditorView {
    /// Run `command` on the text. `Ok(true)` reveals the caret.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_command(
        &mut self,
        ui: &egui::Ui,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        command: EditorCommand,
        now: f64,
    ) -> Result<bool, EditorError> {
        if command.edits() && self.settings.read_only {
            return Ok(true);
        }
        self.sync_others(session, snapshot)?;
        session.end_undo_group();
        match command {
            EditorCommand::SelectLine => {
                let mut carets = self.carets();
                for caret in &mut carets {
                    caret.selection = self.select_line(&snapshot.text, caret.selection);
                    caret.column = None;
                    caret.upstream = false;
                }
                self.set_carets(carets, snapshot.revision);
            }
            EditorCommand::AddCaretAbove => self.add_caret_vertically(ui, snapshot, false),
            EditorCommand::AddCaretBelow => self.add_caret_vertically(ui, snapshot, true),
            EditorCommand::AddNextOccurrence => self.add_next_occurrence(snapshot),
            EditorCommand::SelectAllOccurrences => self.select_all_occurrences(snapshot),
            EditorCommand::AddCaretsToLineEnds => self.carets_at_line_ends(snapshot),
            EditorCommand::ExpandSelection => self.expand_selection(snapshot),
            EditorCommand::ShrinkSelection => self.shrink_selection(snapshot),
            EditorCommand::JumpToBracket => self.jump_to_bracket(snapshot),
            EditorCommand::Fold => self.fold_at_carets(snapshot, folding::FoldAction::Fold),
            EditorCommand::Unfold => self.fold_at_carets(snapshot, folding::FoldAction::Unfold),
            EditorCommand::ToggleFold => self.fold_at_carets(snapshot, folding::FoldAction::Toggle),
            EditorCommand::FoldAll => self.fold_everything(snapshot),
            EditorCommand::UnfoldAll => self.unfold_all(),
            EditorCommand::DeleteLine
            | EditorCommand::CopyLineUp
            | EditorCommand::CopyLineDown
            | EditorCommand::MoveLineUp
            | EditorCommand::MoveLineDown
            | EditorCommand::InsertLineAbove
            | EditorCommand::InsertLineBelow
            | EditorCommand::JoinLines
            | EditorCommand::IndentLines
            | EditorCommand::OutdentLines
            | EditorCommand::ToggleLineComment
            | EditorCommand::ToggleBlockComment => {
                self.run_line_command(id, session, snapshot, command, now)?
            }
        }
        Ok(true)
    }

    /// The caret's line, newline included. A selection grows to its whole
    /// lines, and lines already whole take the next one too. A collapsed
    /// fold is one line.
    fn select_line(&self, text: &TextBuffer, selection: TextSelection) -> TextSelection {
        let range = selection.range();
        let start = text.line_start(self.folding.visible_line(text, range.start));
        let whole_lines = range.start == start && !range.is_empty();
        let end = if !range.is_empty() && range.end == text.line_start(text.line_of(range.end)) {
            if whole_lines {
                self.next_line_start(text, range.end)
            } else {
                range.end
            }
        } else {
            self.next_line_start(text, range.end)
        };
        TextSelection {
            anchor: start,
            head: end,
        }
    }

    /// The start of the line after `byte`'s, past a collapsed fold it opens
    /// or is hidden in (the end of the text on the last line).
    fn next_line_start(&self, text: &TextBuffer, byte: usize) -> usize {
        let line = self.folding.visible_line(text, byte);
        let last = self
            .folding
            .spans(text)
            .filter(|span| text.line_of(span.start) == line)
            .map(|span| span.end)
            .max()
            .map_or(line, |end| text.line_of(end));
        text.line_start(last + 1)
    }
}
